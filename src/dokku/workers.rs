use std::sync::Arc;
use std::time::Duration;

use crate::auth::csrf::generate_token;
use crate::domain::job::{CompletionRefresh, JobPayload};
use crate::storage::runs::{RunOutcome, SqliteRunsRepo};
use crate::web::AppState;

use super::client::DokkuError;

/// Lease duration for a claimed job. The executor heartbeats every
/// [`JOB_HEARTBEAT`], so a silent-but-alive build is never reclaimed; a dead
/// process stops heartbeating and its job is reclaimed after this window.
pub const JOB_LEASE_SECS: i64 = 600;
/// How often the executor refreshes the lease while its command is in flight.
pub const JOB_HEARTBEAT: Duration = Duration::from_secs(60);
/// Retry budget for jobs that are not destructive.
pub const DEFAULT_MAX_ATTEMPTS: usize = 3;
/// Base of the exponential retry backoff (`base * 2^(attempts-1)` seconds).
pub const RETRY_BACKOFF_BASE_SECS: i64 = 5;
/// Cap on the retry backoff.
pub const RETRY_BACKOFF_MAX_SECS: i64 = 300;
/// Poll cadence of the background reclaim worker.
pub const WORKER_POLL: Duration = Duration::from_secs(5);

fn retry_backoff(attempts: usize) -> i64 {
    let exponent = (attempts.max(1) - 1) as u32;
    let multiplier = (1i64 << exponent).min(RETRY_BACKOFF_MAX_SECS / RETRY_BACKOFF_BASE_SECS);
    (multiplier * RETRY_BACKOFF_BASE_SECS).max(1)
}

/// Executes `job_id` to completion (or retry/failure). Safe to call from any
/// process: the SQLite claim ensures only one executor runs the job. The
/// enqueuing process calls this immediately (low latency); the background
/// worker calls it for reclaimed jobs.
/// Executes `job_id` with `owner` as the claim token. The immediate executor
/// passes the job id; the background worker passes its worker id.
pub fn spawn_job_executor(
    state: AppState,
    job_id: String,
    owner: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(execute_job(state, job_id, owner))
}

/// Runs a job to completion (or retry/failure). On the `Claimed` path the
/// executor verifies it owns the claim (`claimed_by == owner`) — proceeding
/// unconditionally would let a stale executor double-run a job another
/// worker holds. On the `Queued` path it claims the job *by id*: never "the
/// oldest", which in a same-second race could be another process's job and
/// strand both.
async fn execute_job(state: AppState, job_id: String, owner: String) {
    let Ok(Some(job)) = state.jobs.get(&job_id).await else {
        tracing::warn!("job {job_id} vanished; skipping");
        return;
    };
    if job.state != crate::storage::jobs::JobState::Queued
        && job.state != crate::storage::jobs::JobState::Claimed
    {
        return;
    }

    // Pre-claim attempt count (the claim increments it for the run about to
    // start), so the retry budget is identical on every execution path.
    let attempts_before = match job.state {
        crate::storage::jobs::JobState::Claimed => job.attempts.saturating_sub(1),
        _ => job.attempts,
    };
    if job.state == crate::storage::jobs::JobState::Queued {
        match state
            .jobs
            .claim_by_id(&job_id, &owner, JOB_LEASE_SECS)
            .await
        {
            Ok(Some(claimed)) if claimed.id == job_id => {}
            Ok(_) => {
                tracing::warn!("job {job_id} is no longer claimable; skipping");
                return;
            }
            Err(err) => {
                tracing::warn!(error = %err, "job claim failed; skipping");
                return;
            }
        }
    } else if job.claimed_by.as_deref() != Some(owner.as_str()) {
        tracing::warn!("job {job_id} is claimed by another worker; skipping");
        return;
    }

    // A run that was already swept (its owner died mid-flight and another
    // process failed it) must not be re-executed: retire the job quietly.
    if let Ok(Some(row)) = state.action_runs.get(&job.run_id).await {
        if row.outcome.is_some() {
            let _ = state.jobs.complete(&job_id).await;
            return;
        }
    }

    let result = run_plan(&state, &job_id, &job.run_id, &job.payload).await;
    match &result {
        Ok(_) => {
            let outcome = RunOutcome {
                ok: true,
                message: job.payload.completion.success_message,
                redirect: job.payload.completion.redirect,
            };
            refresh_after(&state, &job.run_id, job.payload.completion.refresh).await;
            let _ = state.jobs.complete(&job_id).await;
            let _ = state.action_runs.finish(&job.run_id, &outcome).await;
        }
        Err(err) if err.is_transient() && attempts_before + 1 < job.max_attempts => {
            let backoff = retry_backoff(attempts_before);
            let message = err.to_string();
            tracing::warn!(
                attempt = attempts_before,
                backoff = backoff,
                error = %err,
                "job failed transiently; retrying"
            );
            let _ = state.jobs.retry(&job_id, backoff, &message).await;
            let _ = state.action_runs.retry_run(&job.run_id).await;
            let note = format!(
                "-----> command failed ({err}); retrying in {backoff}s (attempt {}/{}).",
                attempts_before + 1,
                job.max_attempts
            );
            let _ = state
                .action_runs
                .append_retry_note(&job.run_id, &note)
                .await;
            // Drive the retry ourselves once the backoff elapses (the claim
            // still enforces single-flight); the background worker is the
            // safety net if this process dies first. Recurse within this task
            // rather than spawning: `execute_job`'s future is `Send` (it is
            // spawned directly elsewhere), but nested spawns here trip the
            // send-bound checker on this particular capture set.
            let retry_state = state.clone();
            let retry_owner = owner.clone();
            tokio::time::sleep(Duration::from_secs(backoff as u64)).await;
            Box::pin(execute_job(retry_state, job_id.clone(), retry_owner)).await;
        }
        Err(err) => {
            let outcome = RunOutcome {
                ok: false,
                message: err.to_string(),
                redirect: None,
            };
            let message = err.to_string();
            let _ = state.jobs.fail(&job_id, &message).await;
            let _ = state.action_runs.finish(&job.run_id, &outcome).await;
        }
    }
}

/// Applies the job's completion refresh policy. `Reports` needs the run's
/// subject (the app name); read from the run row itself.
async fn refresh_after(state: &AppState, run_id: &str, refresh: CompletionRefresh) {
    match refresh {
        CompletionRefresh::Reports => {
            let subject = match state.action_runs.get(run_id).await {
                Ok(Some(row)) => row.subject,
                _ => String::new(),
            };
            if let Err(err) = state.snapshot.refresh_app_reports(&subject).await {
                tracing::warn!(error = %err, "snapshot refresh after job failed");
            }
        }
        CompletionRefresh::All => {
            if let Err(err) = state.snapshot.refresh().await {
                tracing::warn!(error = %err, "snapshot refresh after job failed");
            }
        }
        CompletionRefresh::None => {}
    }
}

/// Executes each step in order, streaming all output into the run. Stops at
/// the first failure. The plan is rehydrated through the newtype constructors,
/// so a tampered payload fails here — without touching the host.
async fn run_plan(
    state: &AppState,
    job_id: &str,
    run_id: &str,
    payload: &JobPayload,
) -> Result<(), DokkuError> {
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
    let line_task = tokio::spawn(persist_lines(
        state.action_runs.clone(),
        run_id.to_owned(),
        rx,
        payload.redactions.clone(),
    ));

    let jobs = state.jobs.clone();
    let runs = state.action_runs.clone();
    let job_id = job_id.to_owned();
    let run_id = run_id.to_owned();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(JOB_HEARTBEAT).await;
            let _ = jobs.heartbeat(&job_id, JOB_LEASE_SECS).await;
            let _ = runs.touch(&run_id).await;
        }
    });

    let mut result: Result<(), DokkuError> = Ok(());
    for spec in &payload.plan {
        let commands = match spec.to_commands() {
            Ok(commands) => commands,
            Err(err) => {
                result = Err(DokkuError::Exit {
                    code: 2,
                    stderr: err.to_string(),
                });
                break;
            }
        };
        let mut failed = false;
        for command in commands {
            match state.dokku.exec_streaming(&command, tx.clone()).await {
                Ok(_) => {}
                Err(err) => {
                    result = Err(err);
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            break;
        }
    }

    heartbeat.abort();
    drop(tx);
    let _ = line_task.await;
    result
}

/// Drains the mpsc channel into the run's persisted lines (mirrors the old
/// spawn_run line task; redaction happens inside `append_line`).
fn persist_lines(
    repo: Arc<SqliteRunsRepo>,
    run_id: String,
    rx: tokio::sync::mpsc::Receiver<String>,
    redactions: Vec<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = rx;
        let mut seq = 0i64;
        let mut partial = String::new();
        while let Some(chunk) = rx.recv().await {
            partial.push_str(&chunk);
            while let Some(pos) = partial.find('\n') {
                let line: String = partial.drain(..=pos).collect();
                let line = line.trim_end_matches(['\n', '\r']).to_owned();
                if let Err(err) = repo
                    .append_line(&run_id, seq as usize, &line, &redactions)
                    .await
                {
                    tracing::warn!(error = %err, "failed to persist run line");
                }
                seq += 1;
            }
        }
        let rest = partial.trim_end_matches(['\n', '\r']);
        if !rest.is_empty() {
            if let Err(err) = repo
                .append_line(&run_id, seq as usize, rest, &redactions)
                .await
            {
                tracing::warn!(error = %err, "failed to persist run line");
            }
        }
    })
}

/// One background worker per call: periodically reclaims expired leases and
/// executes any queued jobs (orphans from dead processes, retried jobs).
/// Every container runs a small pool; the SQLite claim serializes them.
pub fn spawn_worker(state: AppState, worker_id: &str) -> tokio::task::JoinHandle<()> {
    let worker_id = format!(
        "{worker_id}-{}",
        generate_token().chars().take(8).collect::<String>()
    );
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WORKER_POLL).await;
            let _ = state.jobs.reclaim_expired(JOB_LEASE_SECS).await;
            loop {
                let claimed = state.jobs.claim(&worker_id, JOB_LEASE_SECS).await.ok();
                let Some(Some(job)) = claimed else {
                    break;
                };
                execute_job(state.clone(), job.id.clone(), worker_id.clone()).await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_grows_exponentially_and_caps() {
        assert_eq!(retry_backoff(1), 5);
        assert_eq!(retry_backoff(2), 10);
        assert_eq!(retry_backoff(3), 20);
        assert_eq!(retry_backoff(7), 300);
        assert_eq!(retry_backoff(20), 300);
    }
}
