use sqlx::SqlitePool;

use crate::auth::csrf::generate_token;
use crate::domain::job::JobPayload;

/// Job states. `queued` jobs are claimable; `claimed` jobs are being executed
/// under a lease; `done`/`failed` are terminal (pruned with their run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Claimed,
    Done,
    Failed,
}

impl JobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn try_from(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "claimed" => Some(Self::Claimed),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// A claimed job, ready for execution.
#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: String,
    pub run_id: String,
    pub payload: JobPayload,
    pub state: JobState,
    pub attempts: usize,
    pub max_attempts: usize,
    /// Who currently holds the claim (`None` until first claimed). The
    /// executor verifies ownership before running a `Claimed` job, so a
    /// stale executor can never double-run a job another worker holds.
    pub claimed_by: Option<String>,
}

/// Raw row shape for the claim/get queries.
type JobRaw = (String, String, String, String, i64, i64, Option<String>);

/// Shared, persisted job queue. Every container claims from and writes to the
/// same table, so a job enqueued on one container can be executed (or its
/// dead owner's lease reclaimed) by any other — the multi-container backbone
/// of every mutation.
#[derive(Debug, Clone)]
pub struct SqliteJobsRepo {
    pool: SqlitePool,
}

impl SqliteJobsRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Registers a job for `run_id`. `max_attempts` is the total execution
    /// budget (1 = never auto-retry, e.g. destructive operations).
    pub async fn enqueue(
        &self,
        run_id: &str,
        payload: &JobPayload,
        max_attempts: usize,
    ) -> Result<String, sqlx::Error> {
        let id = generate_token();
        let now = now_epoch();
        let data = serde_json::to_string(payload).map_err(serde_err)?;
        sqlx::query(
            "INSERT INTO action_jobs \
             (id, run_id, payload, state, attempts, max_attempts, available_at, \
              lease_expires_at, claimed_by, last_error, created_at, updated_at) \
             VALUES (?, ?, ?, 'queued', 0, ?, ?, NULL, NULL, NULL, ?, ?)",
        )
        .bind(&id)
        .bind(run_id)
        .bind(&data)
        .bind(max_attempts as i64)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Claims the oldest claimable job for `worker_id`, atomically. Runs under
    /// `BEGIN IMMEDIATE` so concurrent claims from different containers
    /// serialize; a job is claimed by exactly one worker. The lease must be
    /// refreshed with [`Self::heartbeat`] or another worker reclaims the job.
    pub async fn claim(
        &self,
        worker_id: &str,
        lease_secs: i64,
    ) -> Result<Option<JobRow>, sqlx::Error> {
        let now = now_epoch();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let result = async {
            let row: Option<JobRaw> = sqlx::query_as(
                "UPDATE action_jobs \
                 SET state = 'claimed', claimed_by = ?, attempts = attempts + 1, \
                     lease_expires_at = ?, updated_at = ? \
                 WHERE id = (SELECT id FROM action_jobs \
                             WHERE state = 'queued' AND available_at <= ? \
                             ORDER BY created_at, id LIMIT 1) \
                 RETURNING id, run_id, payload, state, attempts, max_attempts, claimed_by",
            )
            .bind(worker_id)
            .bind(now + lease_secs)
            .bind(now)
            .bind(now)
            .fetch_optional(&mut *conn)
            .await?;
            Ok(row.and_then(job_from_row))
        }
        .await;
        match result {
            Ok(value) => {
                if let Err(err) = sqlx::query("COMMIT").execute(&mut *conn).await {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    return Err(err);
                }
                Ok(value)
            }
            Err(err) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(err)
            }
        }
    }

    /// Claims a *specific* queued job for `worker_id`, atomically. Used by the
    /// immediate executor, which must only ever run the job it was given —
    /// never "the oldest job", which in a same-second race could be another
    /// process's job. Returns `None` when the job is no longer claimable.
    pub async fn claim_by_id(
        &self,
        job_id: &str,
        worker_id: &str,
        lease_secs: i64,
    ) -> Result<Option<JobRow>, sqlx::Error> {
        let now = now_epoch();
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let result = async {
            let row: Option<JobRaw> = sqlx::query_as(
                "UPDATE action_jobs \
                 SET state = 'claimed', claimed_by = ?, attempts = attempts + 1, \
                     lease_expires_at = ?, updated_at = ? \
                 WHERE id = ? AND state = 'queued' \
                 RETURNING id, run_id, payload, state, attempts, max_attempts, claimed_by",
            )
            .bind(worker_id)
            .bind(now + lease_secs)
            .bind(now)
            .bind(job_id)
            .fetch_optional(&mut *conn)
            .await?;
            Ok(row.and_then(job_from_row))
        }
        .await;
        match result {
            Ok(value) => {
                if let Err(err) = sqlx::query("COMMIT").execute(&mut *conn).await {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    return Err(err);
                }
                Ok(value)
            }
            Err(err) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(err)
            }
        }
    }

    /// Refreshes the lease while the owning worker is still executing, so a
    /// long build is never reclaimed by another worker.
    pub async fn heartbeat(&self, job_id: &str, lease_secs: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE action_jobs SET lease_expires_at = ?, updated_at = ? \
             WHERE id = ? AND state = 'claimed'",
        )
        .bind(now_epoch() + lease_secs)
        .bind(now_epoch())
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn complete(&self, job_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE action_jobs SET state = 'done', updated_at = ? WHERE id = ?")
            .bind(now_epoch())
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn fail(&self, job_id: &str, message: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE action_jobs SET state = 'failed', last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(message)
        .bind(now_epoch())
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Reschedules a transiently-failed job after `backoff_secs`.
    pub async fn retry(
        &self,
        job_id: &str,
        backoff_secs: i64,
        message: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE action_jobs SET state = 'queued', last_error = ?, available_at = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(message)
        .bind(now_epoch() + backoff_secs)
        .bind(now_epoch())
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Requeues jobs whose lease expired (their owner process died), or fails
    /// them when the attempt budget is spent. Returns the count affected.
    /// Counted via `RETURNING` rather than `changes()` (which is
    /// connection-scoped and unreliable across the pooled connections).
    pub async fn reclaim_expired(&self, _lease_secs: i64) -> Result<usize, sqlx::Error> {
        let now = now_epoch();
        let reclaimed: Vec<(String,)> = sqlx::query_as::<_, (String,)>(
            "UPDATE action_jobs \
             SET state = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END, \
                 claimed_by = NULL, lease_expires_at = NULL, \
                 last_error = 'worker lease expired', available_at = ?, updated_at = ? \
             WHERE state = 'claimed' AND lease_expires_at < ? \
             RETURNING id",
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok(reclaimed.len())
    }

    pub async fn get(&self, job_id: &str) -> Result<Option<JobRow>, sqlx::Error> {
        let row: Option<JobRaw> = sqlx::query_as(
            "SELECT id, run_id, payload, state, attempts, max_attempts, claimed_by \
                 FROM action_jobs WHERE id = ?",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(job_from_row))
    }

    /// Active (queued or claimed) jobs for the toast tray.
    pub async fn list_active(&self) -> Result<Vec<JobRow>, sqlx::Error> {
        let rows: Vec<JobRaw> = sqlx::query_as(
            "SELECT id, run_id, payload, state, attempts, max_attempts, claimed_by \
             FROM action_jobs WHERE state IN ('queued', 'claimed') \
             ORDER BY created_at, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(job_from_row)
            .collect::<Vec<_>>())
    }
}

fn job_from_row(
    (id, run_id, payload, state, attempts, max_attempts, claimed_by): JobRaw,
) -> Option<JobRow> {
    let payload: JobPayload = serde_json::from_str(&payload).ok()?;
    let state = JobState::try_from(&state)?;
    Some(JobRow {
        id,
        run_id,
        payload,
        state,
        attempts: attempts as usize,
        max_attempts: max_attempts as usize,
        claimed_by,
    })
}

fn serde_err(err: serde_json::Error) -> sqlx::Error {
    sqlx::Error::AnyDriverError(Box::new(err))
}

fn now_epoch() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::job::{AppAction, CompletionRefresh, CompletionSpec, JobSpec};
    use crate::storage;
    use crate::storage::runs::SqliteRunsRepo;

    fn payload() -> JobPayload {
        JobPayload {
            plan: vec![JobSpec::AppAction {
                app: "alpha".into(),
                action: AppAction::Restart,
            }],
            completion: CompletionSpec {
                success_message: "done".into(),
                redirect: None,
                refresh: CompletionRefresh::None,
            },
        }
    }

    async fn repo() -> (SqliteJobsRepo, SqliteRunsRepo, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/jobs.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (
            SqliteJobsRepo::new(pool.clone()),
            SqliteRunsRepo::new(pool),
            dir,
        )
    }

    /// Enqueues a job against a freshly-created run (the FK requires one).
    async fn enqueue_one(jobs: &SqliteJobsRepo, runs: &SqliteRunsRepo) -> String {
        let run_id = runs.insert("alpha").await.expect("run");
        jobs.enqueue(&run_id, &payload(), 3).await.expect("enqueue")
    }

    #[tokio::test]
    async fn enqueue_and_get_roundtrip() {
        let (jobs, runs, _dir) = repo().await;
        let id = enqueue_one(&jobs, &runs).await;
        let job = jobs.get(&id).await.expect("get").expect("job");
        assert_eq!(job.run_id.len(), 64, "linked to the run");
        assert_eq!(job.state, JobState::Queued);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.max_attempts, 3);
        assert_eq!(job.payload, payload());
    }

    #[tokio::test]
    async fn claim_returns_the_oldest_queued_job_and_marks_it() {
        let (jobs, runs, _dir) = repo().await;
        let first = enqueue_one(&jobs, &runs).await;
        // Backdate the first job so ordering is deterministic (same-second
        // enqueues tiebreak on the random id).
        sqlx::query("UPDATE action_jobs SET created_at = ? WHERE id = ?")
            .bind(1_700_000_000)
            .bind(&first)
            .execute(&jobs.pool)
            .await
            .expect("backdate");
        let second = enqueue_one(&jobs, &runs).await;

        let claimed = jobs
            .claim("worker-a", 600)
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(claimed.id, first, "oldest first");
        assert_eq!(claimed.state, JobState::Claimed);
        assert_eq!(claimed.attempts, 1, "claim counts an attempt");

        // A second worker claims the remaining queued job.
        let second_claim = jobs
            .claim("worker-b", 600)
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(second_claim.id, second);

        assert!(
            jobs.claim("worker-c", 600).await.expect("claim").is_none(),
            "nothing left to claim"
        );
    }

    #[tokio::test]
    async fn claim_is_single_flight_across_workers() {
        let (jobs, runs, _dir) = repo().await;
        let id = enqueue_one(&jobs, &runs).await;

        let first = jobs
            .claim("worker-a", 600)
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(first.id, id);
        assert!(
            jobs.claim("worker-b", 600).await.expect("claim").is_none(),
            "the second worker gets nothing"
        );
    }

    #[tokio::test]
    async fn heartbeat_refreshes_the_lease() {
        let (jobs, runs, _dir) = repo().await;
        let id = enqueue_one(&jobs, &runs).await;
        let _ = jobs.claim("worker-a", 600).await.expect("claim");

        jobs.heartbeat(&id, 600).await.expect("heartbeat");
        let reclaimed = jobs.reclaim_expired(600).await.expect("reclaim");
        assert_eq!(reclaimed, 0, "an active lease is not reclaimed");
    }

    #[tokio::test]
    async fn expired_leases_are_requeued_for_reclaim() {
        let (jobs, runs, _dir) = repo().await;
        let id = enqueue_one(&jobs, &runs).await;
        let _ = jobs.claim("dead-worker", 600).await.expect("claim");

        sqlx::query("UPDATE action_jobs SET lease_expires_at = ? WHERE id = ?")
            .bind(1_700_000_000)
            .bind(&id)
            .execute(&jobs.pool)
            .await
            .expect("backdate lease");

        let reclaimed = jobs.reclaim_expired(600).await.expect("reclaim");
        assert_eq!(reclaimed, 1);
        let job = jobs.get(&id).await.expect("get").expect("job");
        assert_eq!(job.state, JobState::Queued, "back in the pool");
        assert_eq!(job.attempts, 1, "attempts survive the round trip");

        let claimed = jobs
            .claim("fresh-worker", 600)
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.attempts, 2);
    }

    #[tokio::test]
    async fn reclaimed_jobs_past_their_budget_are_failed() {
        let (jobs, runs, _dir) = repo().await;
        let run_id = runs.insert("alpha").await.expect("run");
        let id = jobs.enqueue(&run_id, &payload(), 1).await.expect("enqueue");
        let _ = jobs.claim("dead-worker", 600).await.expect("claim");
        sqlx::query("UPDATE action_jobs SET lease_expires_at = ? WHERE id = ?")
            .bind(1_700_000_000)
            .bind(&id)
            .execute(&jobs.pool)
            .await
            .expect("backdate lease");

        let reclaimed = jobs.reclaim_expired(600).await.expect("reclaim");
        assert_eq!(reclaimed, 1);
        let job = jobs.get(&id).await.expect("get").expect("job");
        assert_eq!(job.state, JobState::Failed, "no retry budget left");
    }

    #[tokio::test]
    async fn retry_requeues_with_a_backoff() {
        let (jobs, runs, _dir) = repo().await;
        let id = enqueue_one(&jobs, &runs).await;
        let _ = jobs.claim("worker-a", 600).await.expect("claim");

        jobs.retry(&id, 30, "transient").await.expect("retry");
        let job = jobs.get(&id).await.expect("get").expect("job");
        assert_eq!(job.state, JobState::Queued);

        assert!(
            jobs.claim("worker-a", 600).await.expect("claim").is_none(),
            "not claimable before its backoff elapses"
        );
        sqlx::query("UPDATE action_jobs SET available_at = ? WHERE id = ?")
            .bind(now_epoch() - 1)
            .bind(&id)
            .execute(&jobs.pool)
            .await
            .expect("elapse backoff");
        let claimed = jobs
            .claim("worker-a", 600)
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.attempts, 2);
    }

    #[tokio::test]
    async fn complete_and_fail_are_terminal() {
        let (jobs, runs, _dir) = repo().await;
        let done = enqueue_one(&jobs, &runs).await;
        let failed = enqueue_one(&jobs, &runs).await;

        let _ = jobs.claim("worker-a", 600).await.expect("claim");
        jobs.complete(&done).await.expect("complete");
        assert_eq!(
            jobs.get(&done).await.expect("get").expect("job").state,
            JobState::Done
        );

        let _ = jobs.claim("worker-a", 600).await.expect("claim");
        jobs.fail(&failed, "boom").await.expect("fail");
        assert_eq!(
            jobs.get(&failed).await.expect("get").expect("job").state,
            JobState::Failed
        );
        assert!(
            jobs.claim("worker-a", 600).await.expect("claim").is_none(),
            "terminal jobs are never claimed"
        );
    }

    #[tokio::test]
    async fn list_active_covers_queued_and_claimed_but_not_terminal() {
        let (jobs, runs, _dir) = repo().await;
        let queued = enqueue_one(&jobs, &runs).await;
        let claimed = enqueue_one(&jobs, &runs).await;
        let done = enqueue_one(&jobs, &runs).await;

        let _ = jobs.claim("worker-a", 600).await.expect("claim");
        jobs.complete(&done).await.expect("complete");

        let active = jobs.list_active().await.expect("active");
        assert_eq!(active.len(), 2);
        assert!(active.iter().any(|job| job.id == queued));
        assert!(active.iter().any(|job| job.id == claimed));
    }

    #[test]
    fn job_state_roundtrips() {
        for state in [
            JobState::Queued,
            JobState::Claimed,
            JobState::Done,
            JobState::Failed,
        ] {
            assert_eq!(JobState::try_from(state.as_str()), Some(state));
        }
        assert_eq!(JobState::try_from("nope"), None);
    }
}
