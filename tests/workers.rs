mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::test_state_with_shared_client;

use dokku_ui::dokku::{
    DokkuClient, DokkuError, DokkuOutput, MockClient, spawn_job_executor, spawn_worker,
};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::job::{AppAction, CompletionRefresh, CompletionSpec, JobPayload, JobSpec};
use dokku_ui::storage::runs::{Actor, NewRun, RunOutcome, TargetKind};
use dokku_ui::web::AppState;

fn app_name(name: &str) -> AppName {
    AppName::try_from(name).expect("valid app name")
}

fn seeded_client() -> MockClient {
    MockClient::new().stub(
        DokkuCommand::AppsList,
        Ok(DokkuOutput::ok("=====> My Apps\nalpha")),
    )
}

fn restart_spec() -> JobSpec {
    JobSpec::AppAction {
        app: "alpha".to_owned(),
        action: AppAction::Restart,
    }
}

fn completion() -> CompletionSpec {
    CompletionSpec {
        success_message: "Restarted 'alpha'.".to_owned(),
        redirect: None,
        refresh: CompletionRefresh::None,
    }
}

async fn enqueue(state: &AppState, max_attempts: usize) -> (String, String) {
    let run_id = state
        .action_runs
        .insert_with(&NewRun {
            subject: "alpha".to_owned(),
            operation: "app.restart".to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: None,
                email: None,
            },
            parent_run_id: None,
        })
        .await
        .expect("run");
    let job_id = state
        .jobs
        .enqueue(
            &run_id,
            &JobPayload {
                plan: vec![restart_spec()],
                completion: completion(),
            },
            max_attempts,
        )
        .await
        .expect("job");
    (run_id, job_id)
}

/// Polls the run until it has an outcome, returning it.
async fn wait_for_outcome(state: &AppState, run_id: &str) -> RunOutcome {
    for _ in 0..1500 {
        if let Some(row) = state.action_runs.get(run_id).await.expect("run") {
            if let Some(outcome) = row.outcome {
                return outcome;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("run never finished: {run_id}");
}

/// Fails `PsRestart` with a Connect error the first N times, then succeeds.
struct FlakyRestart {
    inner: MockClient,
    failures_left: AtomicUsize,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl DokkuClient for FlakyRestart {
    async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if matches!(command, DokkuCommand::PsRestart { .. }) {
            let left = self.failures_left.fetch_sub(1, Ordering::SeqCst);
            if left > 0 {
                return Err(DokkuError::Connect("connection reset".into()));
            }
        }
        self.inner.exec(command).await
    }
}

#[tokio::test]
async fn executor_completes_a_job_streams_lines_and_finishes_the_run() {
    let client = seeded_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> restarting\n-----> done\n")),
    );
    let (state, client_arc, _dir) = test_state_with_shared_client(client).await;
    let (run_id, job_id) = enqueue(&state, 3).await;

    spawn_job_executor(state.clone(), job_id.clone(), job_id.clone());
    let outcome = wait_for_outcome(&state, &run_id).await;

    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.message, "Restarted 'alpha'.");
    let lines = state
        .action_runs
        .lines_after(&run_id, -1)
        .await
        .expect("lines");
    assert_eq!(
        lines,
        vec![
            (0, "-----> restarting".to_owned()),
            (1, "-----> done".to_owned()),
        ]
    );
    let job = state.jobs.get(&job_id).await.expect("job").expect("row");
    assert_eq!(job.state, dokku_ui::storage::jobs::JobState::Done);
    assert!(
        client_arc.calls().contains(&DokkuCommand::PsRestart {
            app: app_name("alpha")
        }),
        "the command ran through the client"
    );
}

#[tokio::test]
async fn executor_does_not_retry_exit_errors() {
    let client = seeded_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Err(DokkuError::Exit {
            code: 1,
            stderr: "app is locked".into(),
        }),
    );
    let (state, client_arc, _dir) = test_state_with_shared_client(client).await;
    let (run_id, job_id) = enqueue(&state, 3).await;

    spawn_job_executor(state.clone(), job_id.clone(), job_id.clone());
    let outcome = wait_for_outcome(&state, &run_id).await;

    assert!(!outcome.ok, "{outcome:?}");
    assert!(outcome.message.contains("app is locked"), "{outcome:?}");
    let calls = client_arc
        .calls()
        .iter()
        .filter(|call| matches!(call, DokkuCommand::PsRestart { .. }))
        .count();
    assert_eq!(calls, 1, "Exit is never auto-retried");
    let job = state.jobs.get(&job_id).await.expect("job").expect("row");
    assert_eq!(job.state, dokku_ui::storage::jobs::JobState::Failed);
}

#[tokio::test]
async fn executor_retries_transient_errors_then_succeeds() {
    let inner = seeded_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> done\n")),
    );
    let client = FlakyRestart {
        inner,
        failures_left: AtomicUsize::new(1),
        calls: AtomicUsize::new(0),
    };
    let (state, _state_b, _dir) =
        common::states_over_shared_db(Arc::new(client), Arc::new(seeded_client())).await;
    let (run_id, job_id) = enqueue(&state, 3).await;

    spawn_job_executor(state.clone(), job_id.clone(), job_id.clone());
    let outcome = wait_for_outcome(&state, &run_id).await;

    assert!(outcome.ok, "retried then succeeded: {outcome:?}");
    let job = state.jobs.get(&job_id).await.expect("job").expect("row");
    assert_eq!(job.state, dokku_ui::storage::jobs::JobState::Done);
    assert!(job.attempts >= 2, "the retry consumed the budget: {job:?}");
    let lines = state
        .action_runs
        .lines_after(&run_id, -1)
        .await
        .expect("lines");
    assert!(
        lines.iter().any(|(_, line)| line.contains("retrying in")),
        "the retry is visible in the run log: {lines:?}"
    );
}

#[tokio::test]
async fn destructive_jobs_are_never_retried() {
    let client = MockClient::new().stub(
        DokkuCommand::AppsDestroy {
            app: app_name("alpha"),
            force: true,
        },
        Err(DokkuError::Connect("down".into())),
    );
    let (state, client_arc, _dir) = test_state_with_shared_client(client).await;
    let run_id = state
        .action_runs
        .insert_with(&NewRun {
            subject: "alpha".to_owned(),
            operation: "app.destroy".to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: None,
                email: None,
            },
            parent_run_id: None,
        })
        .await
        .expect("run");
    let job_id = state
        .jobs
        .enqueue(
            &run_id,
            &JobPayload {
                plan: vec![JobSpec::AppDestroy {
                    app: "alpha".into(),
                }],
                completion: completion(),
            },
            1,
        )
        .await
        .expect("job");

    spawn_job_executor(state.clone(), job_id.clone(), job_id.clone());
    let outcome = wait_for_outcome(&state, &run_id).await;

    assert!(!outcome.ok, "destructive jobs fail outright: {outcome:?}");
    assert_eq!(
        client_arc
            .calls()
            .iter()
            .filter(|call| matches!(call, DokkuCommand::AppsDestroy { .. }))
            .count(),
        1,
        "even a transient error never retries a destroy"
    );
}

#[tokio::test]
async fn a_second_container_reclaims_a_dead_owners_job() {
    // Container A enqueues and its executor claims it but then "dies"
    // mid-run without heartbeating. B is a fully independent AppState over
    // the same SQLite file (two containers behind the proxy).
    let (state_a, state_b, _dir) = common::states_over_shared_db(
        Arc::new(seeded_client()),
        Arc::new(seeded_client().stub(
            DokkuCommand::PsRestart {
                app: app_name("alpha"),
            },
            Ok(DokkuOutput::ok("-----> done\n")),
        )),
    )
    .await;
    let (run_id, job_id) = enqueue(&state_a, 3).await;
    let claimed = state_a
        .jobs
        .claim("exec-dead-owner", 600)
        .await
        .expect("claim")
        .expect("claimed");
    assert_eq!(claimed.id, job_id);

    // Forge an expired lease as if the owner died mid-run.
    sqlx::query("UPDATE action_jobs SET lease_expires_at = ? WHERE id = ?")
        .bind(1_700_000_000)
        .bind(job_id.clone())
        .execute(&state_a.db)
        .await
        .expect("backdate lease");

    // The reclaim worker's loop: reclaim expired leases, then claim + execute.
    let reclaimed = state_b.jobs.reclaim_expired(600).await.expect("reclaim");
    assert_eq!(reclaimed, 1, "B reclaims the orphaned job");
    let job = state_b.jobs.get(&job_id).await.expect("job").expect("row");
    assert_eq!(job.state, dokku_ui::storage::jobs::JobState::Queued);
    let claimed = state_b
        .jobs
        .claim("worker-b", 600)
        .await
        .expect("claim")
        .expect("claimed");
    assert_eq!(claimed.id, job_id);

    // The real worker path (spawn_worker's inner loop) executes it on B.
    spawn_job_executor(state_b.clone(), job_id.clone(), "worker-b".to_owned());
    let outcome = wait_for_outcome(&state_b, &run_id).await;
    assert!(outcome.ok, "{outcome:?}");
    let job = state_b.jobs.get(&job_id).await.expect("job").expect("row");
    assert_eq!(job.state, dokku_ui::storage::jobs::JobState::Done);
}

#[tokio::test]
async fn worker_loop_runs_queued_jobs_itself() {
    let client = seeded_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> done\n")),
    );
    let (state, client_arc, _dir) = test_state_with_shared_client(client).await;
    let (run_id, job_id) = enqueue(&state, 3).await;
    // Do NOT spawn the immediate executor: the worker loop must pick it up.
    let _worker = spawn_worker(state.clone(), "test-worker");

    let outcome = wait_for_outcome(&state, &run_id).await;
    assert!(outcome.ok, "{outcome:?}");
    assert!(client_arc.calls().contains(&DokkuCommand::PsRestart {
        app: app_name("alpha")
    }));
    let job = state.jobs.get(&job_id).await.expect("job").expect("row");
    assert_eq!(job.state, dokku_ui::storage::jobs::JobState::Done);
}

#[tokio::test]
async fn executor_never_runs_a_job_whose_run_was_already_finished() {
    let client = seeded_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> done\n")),
    );
    let (state, client_arc, _dir) = test_state_with_shared_client(client).await;
    let (run_id, job_id) = enqueue(&state, 3).await;
    // A sweeper already failed the run (the owner "died" long ago).
    state
        .action_runs
        .finish(
            &run_id,
            &RunOutcome {
                ok: false,
                message: "Run interrupted (no updates for 10 minutes).".to_owned(),
                redirect: None,
            },
        )
        .await
        .expect("sweep");

    spawn_job_executor(state.clone(), job_id.clone(), job_id.clone());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    assert!(
        client_arc.calls().is_empty(),
        "a swept run is never re-executed"
    );
    let job = state.jobs.get(&job_id).await.expect("job").expect("row");
    assert_eq!(
        job.state,
        dokku_ui::storage::jobs::JobState::Done,
        "retired quietly"
    );
}

#[tokio::test]
async fn same_second_races_never_strand_a_job() {
    // Two jobs enqueued within the same second: each immediate executor must
    // claim its OWN job (never "the oldest", which could be the other's).
    let client = seeded_client().stub(
        DokkuCommand::PsRestart {
            app: app_name("alpha"),
        },
        Ok(DokkuOutput::ok("-----> done\n")),
    );
    let (state, _client_arc, _dir) = test_state_with_shared_client(client).await;
    let (run_a, job_a) = enqueue(&state, 3).await;
    let (run_b, job_b) = enqueue(&state, 3).await;

    spawn_job_executor(state.clone(), job_a.clone(), job_a.clone());
    spawn_job_executor(state.clone(), job_b.clone(), job_b.clone());

    let outcome_a = wait_for_outcome(&state, &run_a).await;
    let outcome_b = wait_for_outcome(&state, &run_b).await;
    assert!(outcome_a.ok, "{outcome_a:?}");
    assert!(outcome_b.ok, "{outcome_b:?}");
    assert_eq!(
        state
            .jobs
            .get(&job_a)
            .await
            .expect("job")
            .expect("row")
            .state,
        dokku_ui::storage::jobs::JobState::Done
    );
    assert_eq!(
        state
            .jobs
            .get(&job_b)
            .await
            .expect("job")
            .expect("row")
            .state,
        dokku_ui::storage::jobs::JobState::Done
    );
}
