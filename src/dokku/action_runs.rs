use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{Mutex, watch};

/// How long a finished run stays queryable so a mid-run page reload can replay it.
const RETAIN_FINISHED: Duration = Duration::from_secs(300);

/// Terminal state of an action run, streamed to the browser as the `done` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunOutcome {
    pub ok: bool,
    pub message: String,
    pub redirect: Option<String>,
}

/// A running (or recently finished) dokku action, with its accumulated output.
#[derive(Debug)]
pub struct ActionRun {
    pub id: u64,
    pub app: String,
    buffer: Mutex<Vec<String>>,
    outcome: Mutex<Option<RunOutcome>>,
    version: watch::Sender<u64>,
    finished_at: Mutex<Option<Instant>>,
}

impl ActionRun {
    /// Appends one complete output line and wakes any followers.
    pub async fn append_line(&self, line: String) {
        self.buffer.lock().await.push(line);
        self.bump();
    }

    /// Records the terminal outcome and wakes any followers.
    pub async fn finish(&self, outcome: RunOutcome) {
        *self.outcome.lock().await = Some(outcome);
        *self.finished_at.lock().await = Some(Instant::now());
        self.bump();
    }

    /// Returns output lines after `cursor` (advancing it) and, if present, the
    /// terminal outcome. Safe to call repeatedly; callers wait on `subscribe`.
    pub async fn poll(&self, cursor: &mut usize) -> (Vec<String>, Option<RunOutcome>) {
        let lines = {
            let buffer = self.buffer.lock().await;
            buffer[*cursor..].to_vec()
        };
        *cursor += lines.len();
        (lines, self.outcome.lock().await.clone())
    }

    /// Subscribes to change notifications (a new line or the final outcome).
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }

    fn bump(&self) {
        let next = *self.version.borrow() + 1;
        let _ = self.version.send(next);
    }
}

/// In-memory registry of action runs. Single-process, like the snapshot store.
#[derive(Debug, Default)]
pub struct ActionRuns {
    runs: Mutex<HashMap<u64, Arc<ActionRun>>>,
    next_id: AtomicU64,
}

impl ActionRuns {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a run, pruning finished runs past their retention window so the
    /// map stays bounded without a sweeper task.
    pub async fn insert(&self, app: &str) -> Arc<ActionRun> {
        self.prune_finished(RETAIN_FINISHED).await;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (version, _) = watch::channel(0);
        let run = Arc::new(ActionRun {
            id,
            app: app.to_owned(),
            buffer: Mutex::new(Vec::new()),
            outcome: Mutex::new(None),
            version,
            finished_at: Mutex::new(None),
        });
        self.runs.lock().await.insert(id, run.clone());
        run
    }

    pub async fn get(&self, id: u64) -> Option<Arc<ActionRun>> {
        self.runs.lock().await.get(&id).cloned()
    }

    /// Drops finished runs older than `grace`.
    pub async fn prune_finished(&self, grace: Duration) {
        let mut runs = self.runs.lock().await;
        let now = Instant::now();
        let mut expired = Vec::new();
        for (id, run) in runs.iter() {
            let finished_at = *run.finished_at.lock().await;
            if let Some(at) = finished_at {
                if now.duration_since(at) >= grace {
                    expired.push(*id);
                }
            }
        }
        for id in expired {
            runs.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(ok: bool) -> RunOutcome {
        RunOutcome {
            ok,
            message: "done".to_owned(),
            redirect: None,
        }
    }

    #[tokio::test]
    async fn insert_assigns_increasing_ids_and_get_returns_run() {
        let runs = ActionRuns::new();

        let first = runs.insert("alpha").await;
        let second = runs.insert("beta").await;

        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
        assert_eq!(first.app, "alpha");
        assert!(Arc::ptr_eq(&runs.get(1).await.expect("run 1"), &first));
        assert!(runs.get(3).await.is_none());
    }

    #[tokio::test]
    async fn poll_replays_lines_once_and_reports_outcome() {
        let runs = ActionRuns::new();
        let run = runs.insert("alpha").await;
        run.append_line("one".to_owned()).await;
        run.append_line("two".to_owned()).await;

        let mut cursor = 0;
        let (lines, outcome) = run.poll(&mut cursor).await;
        assert_eq!(lines, vec!["one".to_owned(), "two".to_owned()]);
        assert_eq!(outcome, None);

        let (lines, _) = run.poll(&mut cursor).await;
        assert!(lines.is_empty(), "cursor advanced past delivered lines");

        run.finish(done(true)).await;
        let (lines, outcome) = run.poll(&mut cursor).await;
        assert!(lines.is_empty());
        assert_eq!(outcome, Some(done(true)));
    }

    #[tokio::test]
    async fn subscribe_notifies_on_lines_and_finish() {
        let runs = ActionRuns::new();
        let run = runs.insert("alpha").await;
        let mut rx = run.subscribe();

        run.append_line("x".to_owned()).await;
        assert!(rx.changed().await.is_ok(), "line wakes followers");

        run.finish(done(true)).await;
        assert!(rx.changed().await.is_ok(), "finish wakes followers");
    }

    #[tokio::test]
    async fn prune_drops_finished_runs_past_grace_only() {
        let runs = ActionRuns::new();
        let finished = runs.insert("alpha").await;
        let running = runs.insert("beta").await;
        finished.finish(done(true)).await;

        runs.prune_finished(Duration::from_secs(300)).await;
        assert!(runs.get(finished.id).await.is_some(), "within grace");
        assert!(runs.get(running.id).await.is_some(), "still running");

        runs.prune_finished(Duration::ZERO).await;
        assert!(runs.get(finished.id).await.is_none(), "past grace");
        assert!(runs.get(running.id).await.is_some(), "running never pruned");
    }
}
