use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth::csrf::generate_token;

/// Finished runs stay queryable for a mid-run page reload, then are pruned.
pub const RETAIN_FINISHED_SECS: i64 = 300;
/// Running runs whose owner process died stop bumping `updated_at`; past this
/// window they are marked failed so followers and the UI can move on.
pub const ORPHAN_AFTER_SECS: i64 = 600;

/// Terminal state of an action run, streamed to the browser as the `done` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub ok: bool,
    pub message: String,
    pub redirect: Option<String>,
}

/// Synthetic outcome for runs whose owning process disappeared mid-stream.
pub fn interrupted_outcome() -> RunOutcome {
    RunOutcome {
        ok: false,
        message: format!(
            "Run interrupted (no updates for {} minutes).",
            ORPHAN_AFTER_SECS / 60
        ),
        redirect: None,
    }
}

#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: String,
    pub subject: String,
    pub outcome: Option<RunOutcome>,
    pub updated_at: i64,
}

/// Shared, persisted registry of action runs. Every container can start runs
/// and follow anyone's run, which is what makes `ps:scale web=N` safe.
#[derive(Debug, Clone)]
pub struct SqliteRunsRepo {
    pool: SqlitePool,
    retain_finished_secs: i64,
    orphan_after_secs: i64,
}

impl SqliteRunsRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self::with_policy(pool, RETAIN_FINISHED_SECS, ORPHAN_AFTER_SECS)
    }

    /// Policy-injectable constructor so tests can use tiny sweep windows.
    pub fn with_policy(
        pool: SqlitePool,
        retain_finished_secs: i64,
        orphan_after_secs: i64,
    ) -> Self {
        Self {
            pool,
            retain_finished_secs,
            orphan_after_secs,
        }
    }

    /// Registers a new run and returns its id. Piggybacks the two table sweeps
    /// (prune finished, fail orphaned) so the tables stay bounded with no
    /// background task.
    pub async fn insert(&self, subject: &str) -> Result<String, sqlx::Error> {
        self.prune_finished().await?;
        self.sweep_orphans().await?;
        let id = generate_token();
        let now = now_epoch();
        sqlx::query(
            "INSERT INTO action_runs (id, subject, outcome, created_at, updated_at, finished_at) \
             VALUES (?, ?, NULL, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(subject)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Appends one line (with its seq) and bumps the run's heartbeat.
    pub async fn append_line(
        &self,
        run_id: &str,
        seq: usize,
        line: &str,
    ) -> Result<(), sqlx::Error> {
        let now = now_epoch();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO action_run_lines (run_id, seq, line) VALUES (?, ?, ?)")
            .bind(run_id)
            .bind(seq as i64)
            .bind(line)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE action_runs SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// Bumps the heartbeat while the owning command is still running, so a
    /// quiet-but-alive build (long image pull, no output) is never mistaken
    /// for an orphan. No-op once the run has an outcome.
    pub async fn touch(&self, run_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE action_runs SET updated_at = ? WHERE id = ? AND outcome IS NULL")
            .bind(now_epoch())
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn finish(&self, run_id: &str, outcome: &RunOutcome) -> Result<(), sqlx::Error> {
        let now = now_epoch();
        let data = serde_json::to_string(outcome).map_err(serde_err)?;
        sqlx::query(
            "UPDATE action_runs SET outcome = ?, finished_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(data)
        .bind(now)
        .bind(now)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, run_id: &str) -> Result<Option<RunRow>, sqlx::Error> {
        let row: Option<(String, String, Option<String>, i64)> =
            sqlx::query_as("SELECT id, subject, outcome, updated_at FROM action_runs WHERE id = ?")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some((id, subject, outcome_json, updated_at)) = row else {
            return Ok(None);
        };
        let outcome = match outcome_json {
            None => None,
            Some(json) => match serde_json::from_str(&json) {
                Ok(outcome) => Some(outcome),
                Err(err) => {
                    tracing::warn!(error = %err, "run outcome is corrupt; treating as running");
                    None
                }
            },
        };
        Ok(Some(RunRow {
            id,
            subject,
            outcome,
            updated_at,
        }))
    }

    /// Lines strictly after `after_seq`, in order. Pass `-1` to replay everything.
    pub async fn lines_after(
        &self,
        run_id: &str,
        after_seq: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT seq, line FROM action_run_lines WHERE run_id = ? AND seq > ? ORDER BY seq",
        )
        .bind(run_id)
        .bind(after_seq)
        .fetch_all(&self.pool)
        .await
    }

    async fn prune_finished(&self) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM action_runs WHERE finished_at IS NOT NULL AND finished_at < ?")
            .bind(now_epoch() - self.retain_finished_secs)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn sweep_orphans(&self) -> Result<(), sqlx::Error> {
        let now = now_epoch();
        let data = serde_json::to_string(&interrupted_outcome()).map_err(serde_err)?;
        sqlx::query(
            "UPDATE action_runs SET outcome = ?, finished_at = ?, updated_at = ? \
             WHERE outcome IS NULL AND updated_at < ?",
        )
        .bind(data)
        .bind(now)
        .bind(now)
        .bind(now - self.orphan_after_secs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
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
    use crate::storage;

    async fn repo_with(retain: i64, orphan: i64) -> (SqliteRunsRepo, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/runs.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteRunsRepo::with_policy(pool, retain, orphan), dir)
    }

    fn done(ok: bool) -> RunOutcome {
        RunOutcome {
            ok,
            message: "done".to_owned(),
            redirect: None,
        }
    }

    async fn backdate(repo: &SqliteRunsRepo, id: &str, seconds: i64) {
        sqlx::query("UPDATE action_runs SET updated_at = ?, finished_at = ? WHERE id = ?")
            .bind(now_epoch() - seconds)
            .bind(now_epoch() - seconds)
            .bind(id)
            .execute(&repo.pool)
            .await
            .expect("backdate");
    }

    async fn backdate_heartbeat(repo: &SqliteRunsRepo, id: &str, seconds: i64) {
        sqlx::query("UPDATE action_runs SET updated_at = ? WHERE id = ?")
            .bind(now_epoch() - seconds)
            .bind(id)
            .execute(&repo.pool)
            .await
            .expect("backdate heartbeat");
    }

    #[tokio::test]
    async fn insert_returns_distinct_64_hex_ids() {
        let (repo, _dir) = repo_with(300, 600).await;
        let first = repo.insert("alpha").await.expect("insert");
        let second = repo.insert("beta").await.expect("insert");
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn insert_roundtrips_via_get() {
        let (repo, _dir) = repo_with(300, 600).await;
        let id = repo.insert("alpha").await.expect("insert");
        let row = repo.get(&id).await.expect("get").expect("row");
        assert_eq!(row.subject, "alpha");
        assert_eq!(row.outcome, None);
    }

    #[tokio::test]
    async fn get_unknown_id_returns_none() {
        let (repo, _dir) = repo_with(300, 600).await;
        assert!(repo.get("nope").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn append_line_orders_and_advances_cursors() {
        let (repo, _dir) = repo_with(300, 600).await;
        let id = repo.insert("alpha").await.expect("insert");
        repo.append_line(&id, 0, "one").await.expect("line");
        repo.append_line(&id, 1, "two").await.expect("line");
        repo.append_line(&id, 2, "three").await.expect("line");

        assert_eq!(
            repo.lines_after(&id, -1).await.expect("all"),
            vec![(0, "one".into()), (1, "two".into()), (2, "three".into()),]
        );
        assert_eq!(
            repo.lines_after(&id, 1).await.expect("tail"),
            vec![(2, "three".into())]
        );
        assert!(repo.lines_after(&id, 2).await.expect("done").is_empty());
    }

    #[tokio::test]
    async fn lines_after_unknown_run_is_empty() {
        let (repo, _dir) = repo_with(300, 600).await;
        assert!(
            repo.lines_after("nope", -1)
                .await
                .expect("lines")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn finish_is_visible_via_get() {
        let (repo, _dir) = repo_with(300, 600).await;
        let id = repo.insert("alpha").await.expect("insert");
        let outcome = done(true);
        repo.finish(&id, &outcome).await.expect("finish");
        assert_eq!(
            repo.get(&id).await.expect("get").expect("row").outcome,
            Some(outcome)
        );
    }

    #[tokio::test]
    async fn insert_prunes_finished_runs_past_grace() {
        let (repo, _dir) = repo_with(0, 3600).await;
        let old = repo.insert("alpha").await.expect("insert");
        repo.finish(&old, &done(true)).await.expect("finish");
        backdate(&repo, &old, 1000).await;
        let fresh = repo.insert("beta").await.expect("insert");
        assert!(
            repo.get(&old).await.expect("get").is_none(),
            "finished past grace pruned"
        );
        assert!(
            repo.get(&fresh).await.expect("get").is_some(),
            "running kept"
        );
    }

    #[tokio::test]
    async fn insert_sweeps_stale_running_runs() {
        let (repo, _dir) = repo_with(300, 0).await;
        let stale = repo.insert("alpha").await.expect("insert");
        backdate_heartbeat(&repo, &stale, 1000).await;
        let _fresh = repo.insert("beta").await.expect("insert");
        let row = repo.get(&stale).await.expect("get").expect("row");
        let outcome = row.outcome.expect("swept to a failure outcome");
        assert!(!outcome.ok);
    }

    #[tokio::test]
    async fn sweep_skips_recently_active_runs() {
        let (repo, _dir) = repo_with(300, 0).await;
        let active = repo.insert("alpha").await.expect("insert");
        sqlx::query("UPDATE action_runs SET updated_at = ? WHERE id = ?")
            .bind(now_epoch() + 1000)
            .bind(&active)
            .execute(&repo.pool)
            .await
            .expect("bump");
        let _fresh = repo.insert("beta").await.expect("insert");
        assert_eq!(
            repo.get(&active).await.expect("get").expect("row").outcome,
            None,
            "recently active run is not swept"
        );
    }

    #[tokio::test]
    async fn touch_keeps_a_quiet_run_alive() {
        let (repo, _dir) = repo_with(300, 0).await;
        let quiet = repo.insert("alpha").await.expect("insert");
        backdate_heartbeat(&repo, &quiet, 1000).await;
        repo.touch(&quiet).await.expect("touch");
        let _fresh = repo.insert("beta").await.expect("insert");
        assert_eq!(
            repo.get(&quiet).await.expect("get").expect("row").outcome,
            None,
            "a heartbeated run is not swept, even with no output"
        );
    }

    #[tokio::test]
    async fn touch_is_a_noop_once_finished() {
        let (repo, _dir) = repo_with(300, 600).await;
        let run = repo.insert("alpha").await.expect("insert");
        repo.finish(&run, &done(true)).await.expect("finish");
        let before = repo.get(&run).await.expect("get").expect("row").updated_at;
        repo.touch(&run).await.expect("touch");
        let after = repo.get(&run).await.expect("get").expect("row");
        assert_eq!(
            after.updated_at, before,
            "finished runs are not resurrected"
        );
        assert!(after.outcome.is_some());
    }

    #[tokio::test]
    async fn pruning_a_run_cascades_its_lines() {
        let (repo, _dir) = repo_with(0, 3600).await;
        let doomed = repo.insert("alpha").await.expect("insert");
        repo.append_line(&doomed, 0, "x").await.expect("line");
        repo.finish(&doomed, &done(true)).await.expect("finish");
        backdate(&repo, &doomed, 1000).await;
        let _other = repo.insert("beta").await.expect("insert");
        assert!(
            repo.lines_after(&doomed, -1)
                .await
                .expect("lines")
                .is_empty()
        );
    }

    #[test]
    fn interrupted_outcome_is_a_failure() {
        let outcome = interrupted_outcome();
        assert!(!outcome.ok);
        assert!(outcome.message.contains("interrupted"));
        assert_eq!(outcome.redirect, None);
    }
}
