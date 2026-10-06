use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth::csrf::generate_token;
use crate::domain::redact::redact_line;

/// How long finished runs keep their metadata for the audit trail.
pub const ACTIVITY_TTL_SECS: i64 = 7_776_000; // 90 days
/// How long finished runs keep their streamed lines (the metadata — actor,
/// operation, outcome — outlives them).
pub const LOG_TTL_SECS: i64 = 604_800; // 7 days
/// Running runs whose owner process died stop bumping `updated_at`; past this
/// window they are marked failed so followers and the UI can move on.
pub const ORPHAN_AFTER_SECS: i64 = 600;

/// Retention policy for runs and their lines. Inject for tests (tiny windows)
/// and from settings in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunPolicy {
    /// Runs are pruned this long after finishing (metadata + lines cascade).
    pub activity_ttl_secs: i64,
    /// Lines of finished runs are pruned this long after finishing; the run
    /// row itself survives until the activity TTL.
    pub log_ttl_secs: i64,
    /// Running runs with no heartbeat or output past this window are failed.
    pub orphan_after_secs: i64,
}

impl RunPolicy {
    /// The production retention policy (90-day activity, 7-day logs, 10-minute
    /// orphan window). Tests inject smaller windows via `with_policy`.
    pub fn standard() -> Self {
        Self {
            activity_ttl_secs: ACTIVITY_TTL_SECS,
            log_ttl_secs: LOG_TTL_SECS,
            orphan_after_secs: ORPHAN_AFTER_SECS,
        }
    }
}

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

/// Who (or what) initiated a run. `None`s mean the action came from the
/// system (background sweeps, orphan reclaim) or attribution failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub user_id: Option<i64>,
    pub email: Option<String>,
}

/// The audit dimensions of a run, recorded at insert time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    /// Target name: app name, service name, or volume entry.
    pub subject: String,
    /// Stable machine key, e.g. `app.restart`, `service.create`, `volume.mount`.
    pub operation: String,
    pub target_kind: TargetKind,
    pub actor: Actor,
    /// Parent run id for lineage (e.g. a rebuild spawned by a config edit).
    pub parent_run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    App,
    Service,
    Volume,
    System,
}

impl TargetKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Service => "service",
            Self::Volume => "volume",
            Self::System => "system",
        }
    }

    pub fn try_from(value: &str) -> Option<Self> {
        match value {
            "app" => Some(Self::App),
            "service" => Some(Self::Service),
            "volume" => Some(Self::Volume),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: String,
    pub subject: String,
    pub outcome: Option<RunOutcome>,
    pub updated_at: i64,
}

/// One row of the audit trail, as shown on the activity pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub id: String,
    pub operation: String,
    pub target_kind: TargetKind,
    pub subject: String,
    pub actor_email: Option<String>,
    pub ok: Option<bool>,
    pub created_at: i64,
}

impl RunSummary {
    /// Human-friendly "when" for the activity table.
    pub fn formatted_when(&self) -> String {
        format_when(self.created_at)
    }

    pub fn actor_email_label(&self) -> String {
        match &self.actor_email {
            Some(email) => email.clone(),
            None => "system".to_owned(),
        }
    }

    pub fn outcome_label(&self) -> String {
        match self.ok {
            Some(true) => "succeeded".to_owned(),
            Some(false) => "failed".to_owned(),
            None => "running".to_owned(),
        }
    }
}

/// "just now" / "5m ago" / "3h ago" / "2d ago", falling back to the UTC date
/// for anything older than a week.
pub fn format_when(created_at: i64) -> String {
    let now = now_epoch();
    let age = (now - created_at).max(0) as u64;
    if age < 60 {
        return "just now".to_owned();
    }
    if age < 3600 {
        return format!("{}m ago", age / 60);
    }
    if age < 86400 {
        return format!("{}h ago", age / 3600);
    }
    if age < 604_800 {
        return format!("{}d ago", age / 86400);
    }
    let Ok(datetime) = time::OffsetDateTime::from_unix_timestamp(created_at) else {
        return "—".to_owned();
    };
    format!(
        "{:04}-{:02}-{:02}",
        datetime.year(),
        u8::from(datetime.month()),
        datetime.day()
    )
}

/// Raw row shape for the summary queries.
type SummaryRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
);

fn summaries_from_rows(rows: Vec<SummaryRow>) -> Vec<RunSummary> {
    rows.into_iter()
        .filter_map(
            |(id, operation, kind, subject, actor_email, outcome, created_at)| {
                let target_kind = TargetKind::try_from(&kind)?;
                Some(RunSummary {
                    id,
                    operation,
                    target_kind,
                    subject,
                    actor_email,
                    ok: parse_outcome(outcome).map(|outcome| outcome.ok),
                    created_at,
                })
            },
        )
        .collect::<Vec<_>>()
}

/// Shared, persisted registry of action runs. Every container can start runs
/// and follow anyone's run, which is what makes `ps:scale web=N` safe.
#[derive(Debug, Clone)]
pub struct SqliteRunsRepo {
    pool: SqlitePool,
    policy: RunPolicy,
}

impl SqliteRunsRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self::with_policy(pool, RunPolicy::standard())
    }

    /// Policy-injectable constructor so tests can use tiny sweep windows.
    pub fn with_policy(pool: SqlitePool, policy: RunPolicy) -> Self {
        Self { pool, policy }
    }

    /// Registers a run with default audit dimensions (`unknown`, `system`).
    /// Convenience for tests; production callers use [`Self::insert_with`].
    pub async fn insert(&self, subject: &str) -> Result<String, sqlx::Error> {
        self.insert_with(&NewRun {
            subject: subject.to_owned(),
            operation: "unknown".to_owned(),
            target_kind: TargetKind::System,
            actor: Actor {
                user_id: None,
                email: None,
            },
            parent_run_id: None,
        })
        .await
    }

    /// Registers a run and returns its id. Piggybacks the three table sweeps
    /// (prune finished by activity TTL, prune lines by log TTL, fail orphans)
    /// so the tables stay bounded with no background task.
    pub async fn insert_with(&self, new_run: &NewRun) -> Result<String, sqlx::Error> {
        self.prune_finished().await?;
        self.prune_lines().await?;
        self.sweep_orphans().await?;
        let id = generate_token();
        let now = now_epoch();
        sqlx::query(
            "INSERT INTO action_runs \
             (id, subject, operation, target_kind, actor_user_id, actor_email, parent_run_id, \
              outcome, created_at, updated_at, finished_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(&new_run.subject)
        .bind(&new_run.operation)
        .bind(new_run.target_kind.as_str())
        .bind(new_run.actor.user_id)
        .bind(new_run.actor.email.clone())
        .bind(new_run.parent_run_id.clone())
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Appends one line (with its seq) and bumps the run's heartbeat. Lines
    /// are redacted before persisting so secrets never land in the audit log.
    pub async fn append_line(
        &self,
        run_id: &str,
        seq: usize,
        line: &str,
        redactions: &[String],
    ) -> Result<(), sqlx::Error> {
        let now = now_epoch();
        let line = redact_line(line, redactions);
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO action_run_lines (run_id, seq, line) VALUES (?, ?, ?)")
            .bind(run_id)
            .bind(seq as i64)
            .bind(&line)
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

    /// Appends a synthetic line at the next free seq (used by the job queue to
    /// announce retries between attempts).
    pub async fn append_retry_note(&self, run_id: &str, note: &str) -> Result<(), sqlx::Error> {
        let last: Option<(i64,)> = sqlx::query_as(
            "SELECT seq FROM action_run_lines WHERE run_id = ? ORDER BY seq DESC LIMIT 1",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        let seq = last.map(|(seq,)| seq).unwrap_or(-1) + 1;
        self.append_line(run_id, seq as usize, note, &[]).await
    }

    /// Resets a run that is about to be retried: clears any terminal outcome
    /// so the SSE stream keeps following it.
    pub async fn retry_run(&self, run_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE action_runs SET outcome = NULL, finished_at = NULL, updated_at = ? \
             WHERE id = ?",
        )
        .bind(now_epoch())
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
        Ok(Some(RunRow {
            id,
            subject,
            outcome: parse_outcome(outcome_json),
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

    /// Newest-first audit trail, bounded by `limit`.
    pub async fn list_recent(&self, limit: usize) -> Result<Vec<RunSummary>, sqlx::Error> {
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT id, operation, target_kind, subject, actor_email, outcome, created_at \
             FROM action_runs ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(summaries_from_rows(rows))
    }

    pub async fn list_for_target(
        &self,
        kind: TargetKind,
        name: &str,
        limit: usize,
    ) -> Result<Vec<RunSummary>, sqlx::Error> {
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT id, operation, target_kind, subject, actor_email, outcome, created_at \
             FROM action_runs WHERE target_kind = ? AND subject = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(kind.as_str())
        .bind(name)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(summaries_from_rows(rows))
    }

    pub async fn list_for_actor_email(
        &self,
        email: &str,
        limit: usize,
    ) -> Result<Vec<RunSummary>, sqlx::Error> {
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT id, operation, target_kind, subject, actor_email, outcome, created_at \
             FROM action_runs WHERE actor_email = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(email)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(summaries_from_rows(rows))
    }

    /// Newest-first runs started by `user_id` (used by the toast tray).
    pub async fn list_for_actor(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<RunSummary>, sqlx::Error> {
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT id, operation, target_kind, subject, actor_email, outcome, created_at \
             FROM action_runs WHERE actor_user_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(summaries_from_rows(rows))
    }

    /// Records that `user_id` dismissed the completion toast for `run_id`.
    pub async fn ack(&self, run_id: &str, user_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO action_run_acks (run_id, user_id, acknowledged_at) VALUES (?, ?, ?) \
             ON CONFLICT(run_id, user_id) DO NOTHING",
        )
        .bind(run_id)
        .bind(user_id)
        .bind(now_epoch())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// All run ids `user_id` has acknowledged.
    pub async fn acked_run_ids(&self, user_id: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT run_id FROM action_run_acks WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(run_id,)| run_id).collect::<Vec<_>>())
    }

    /// Subjects with an unfinished app destroy run; the dashboard marks these
    /// rows as "deleting" until the run lands. Scoped to `app.destroy` so a
    /// service whose name collides with an app never shows a phantom badge.
    pub async fn active_destruction_subjects(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT subject FROM action_runs WHERE outcome IS NULL AND operation = 'app.destroy'",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(subject,)| subject).collect())
    }

    async fn prune_finished(&self) -> Result<(), sqlx::Error> {
        let ttl = self
            .effective_ttl_secs("activity_ttl_secs", self.policy.activity_ttl_secs)
            .await;
        sqlx::query("DELETE FROM action_runs WHERE finished_at IS NOT NULL AND finished_at < ?")
            .bind(now_epoch() - ttl)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn prune_lines(&self) -> Result<(), sqlx::Error> {
        let ttl = self
            .effective_ttl_secs("run_log_ttl_secs", self.policy.log_ttl_secs)
            .await;
        sqlx::query(
            "DELETE FROM action_run_lines WHERE run_id IN \
             (SELECT id FROM action_runs WHERE finished_at IS NOT NULL AND finished_at < ?)",
        )
        .bind(now_epoch() - ttl)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Instance-settings TTL override, falling back to the injected policy when
    /// the admin has not set one (or the row is tampered).
    async fn effective_ttl_secs(&self, key: &str, fallback: i64) -> i64 {
        sqlx::query_scalar::<_, String>("SELECT value FROM instance_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .and_then(|raw| raw.parse::<i64>().ok())
            .filter(|secs| (60..=crate::domain::TTL_MAX_SECS as i64).contains(secs))
            .unwrap_or(fallback)
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
        .bind(now - self.policy.orphan_after_secs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn parse_outcome(outcome_json: Option<String>) -> Option<RunOutcome> {
    match outcome_json {
        None => None,
        Some(json) => match serde_json::from_str(&json) {
            Ok(outcome) => Some(outcome),
            Err(err) => {
                tracing::warn!(error = %err, "run outcome is corrupt; treating as running");
                None
            }
        },
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

    async fn repo_with(activity_ttl: i64, orphan: i64) -> (SqliteRunsRepo, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/runs.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        let policy = RunPolicy {
            activity_ttl_secs: activity_ttl,
            log_ttl_secs: activity_ttl,
            orphan_after_secs: orphan,
        };
        (SqliteRunsRepo::with_policy(pool, policy), dir)
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
        repo.append_line(&id, 0, "one", &[]).await.expect("line");
        repo.append_line(&id, 1, "two", &[]).await.expect("line");
        repo.append_line(&id, 2, "three", &[]).await.expect("line");

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
    async fn append_line_redacts_secrets_before_persisting() {
        let (repo, _dir) = repo_with(300, 600).await;
        let id = repo.insert("alpha").await.expect("insert");
        repo.append_line(&id, 0, "DATABASE_URL: postgres://u:p@h/db", &[])
            .await
            .expect("line");
        repo.append_line(&id, 1, "Setting FOO to hunter2", &["hunter2".into()])
            .await
            .expect("line");

        let lines = repo.lines_after(&id, -1).await.expect("lines");
        assert!(
            !lines[0].1.contains("postgres://"),
            "URL credentials never persist"
        );
        assert!(
            !lines[1].1.contains("hunter2"),
            "literal secrets never persist"
        );
        assert!(lines[1].1.contains(crate::domain::redact::MASK));
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
    async fn instance_settings_override_the_retention_policy() {
        let (repo, _dir) = repo_with(3600, 3600).await;
        let old = repo.insert("alpha").await.expect("insert");
        repo.finish(&old, &done(true)).await.expect("finish");
        backdate(&repo, &old, 1000).await;

        sqlx::query(
            "INSERT INTO instance_settings (key, value, updated_at) \
             VALUES ('activity_ttl_secs', '60', 0)",
        )
        .execute(&repo.pool)
        .await
        .expect("override");

        let _fresh = repo.insert("beta").await.expect("insert");
        assert!(
            repo.get(&old).await.expect("get").is_none(),
            "the instance override prunes the run"
        );
    }

    #[tokio::test]
    async fn finished_metadata_outlives_lines_until_the_log_ttl() {
        // Runs are retained for 3600s; their lines are pruned immediately
        // after finishing (log TTL 0).
        let (repo, dir) = {
            let dir = tempfile::TempDir::new().expect("temp dir");
            let url = format!("sqlite://{}/runs.db", dir.path().display());
            let pool = storage::connect(&url).await.expect("connect");
            let policy = RunPolicy {
                activity_ttl_secs: 3600,
                log_ttl_secs: 0,
                orphan_after_secs: 3600,
            };
            (SqliteRunsRepo::with_policy(pool, policy), dir)
        };
        let id = repo.insert("alpha").await.expect("insert");
        repo.append_line(&id, 0, "old line", &[])
            .await
            .expect("line");
        repo.finish(&id, &done(true)).await.expect("finish");
        sqlx::query("UPDATE action_runs SET finished_at = ? WHERE id = ?")
            .bind(now_epoch() - 1000)
            .bind(&id)
            .execute(&repo.pool)
            .await
            .expect("backdate finish");

        let fresh = repo.insert("beta").await.expect("insert");

        assert!(
            repo.get(&id).await.expect("get").is_some(),
            "metadata survives past the log TTL"
        );
        assert!(
            repo.lines_after(&id, -1).await.expect("lines").is_empty(),
            "lines pruned past the log TTL"
        );
        assert!(
            repo.get(&fresh).await.expect("get").is_some(),
            "a fresh insert stays intact"
        );
        assert!(dir.path().is_dir(), "temp dir kept for the repo's lifetime");
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
        repo.append_line(&doomed, 0, "x", &[]).await.expect("line");
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

    #[tokio::test]
    async fn insert_with_records_audit_dimensions() {
        let (repo, _dir) = repo_with(300, 600).await;
        let new_run = NewRun {
            subject: "alpha".to_owned(),
            operation: "app.restart".to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: Some(7),
                email: Some("ops@example.com".to_owned()),
            },
            parent_run_id: None,
        };
        let id = repo.insert_with(&new_run).await.expect("insert");

        let recent = repo.list_recent(10).await.expect("recent");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, id);
        assert_eq!(recent[0].operation, "app.restart");
        assert_eq!(recent[0].target_kind, TargetKind::App);
        assert_eq!(recent[0].subject, "alpha");
        assert_eq!(recent[0].actor_email, Some("ops@example.com".to_owned()));
        assert_eq!(recent[0].ok, None);
    }

    #[tokio::test]
    async fn summaries_filter_by_target_and_actor() {
        let (repo, _dir) = repo_with(300, 600).await;
        let app_run = repo
            .insert_with(&NewRun {
                subject: "alpha".to_owned(),
                operation: "app.restart".to_owned(),
                target_kind: TargetKind::App,
                actor: Actor {
                    user_id: Some(1),
                    email: Some("ops@example.com".to_owned()),
                },
                parent_run_id: None,
            })
            .await
            .expect("app run");
        // Backdate the first run so ordering is deterministic (same-second
        // inserts tiebreak on the random id).
        sqlx::query("UPDATE action_runs SET created_at = ? WHERE id = ?")
            .bind(1_700_000_000)
            .bind(&app_run)
            .execute(&repo.pool)
            .await
            .expect("backdate");
        let svc_run = repo
            .insert_with(&NewRun {
                subject: "candid".to_owned(),
                operation: "service.stop".to_owned(),
                target_kind: TargetKind::Service,
                actor: Actor {
                    user_id: Some(1),
                    email: Some("ops@example.com".to_owned()),
                },
                parent_run_id: None,
            })
            .await
            .expect("service run");

        let app_rows = repo
            .list_for_target(TargetKind::App, "alpha", 10)
            .await
            .expect("app rows");
        assert_eq!(app_rows.len(), 1);
        assert_eq!(app_rows[0].id, app_run);

        let actor_rows = repo
            .list_for_actor_email("ops@example.com", 10)
            .await
            .expect("actor rows");
        assert_eq!(actor_rows.len(), 2);
        assert_eq!(actor_rows[0].id, svc_run, "newest first");
        assert_eq!(actor_rows[1].id, app_run);

        let other_rows = repo
            .list_for_actor_email("nobody@example.com", 10)
            .await
            .expect("other actor");
        assert!(other_rows.is_empty());

        let svc_rows = repo
            .list_for_target(TargetKind::Service, "candid", 10)
            .await
            .expect("service rows");
        assert_eq!(svc_rows.len(), 1);
        assert_eq!(svc_rows[0].id, svc_run);
    }

    #[tokio::test]
    async fn summaries_reflect_outcomes() {
        let (repo, _dir) = repo_with(300, 600).await;
        let ok_run = repo.insert("alpha").await.expect("insert");
        repo.finish(&ok_run, &done(true)).await.expect("finish ok");
        let bad_run = repo.insert("beta").await.expect("insert");
        repo.finish(&bad_run, &done(false))
            .await
            .expect("finish bad");

        let recent = repo.list_recent(10).await.expect("recent");
        let ok_row = recent.iter().find(|row| row.id == ok_run).expect("ok row");
        assert_eq!(ok_row.ok, Some(true));
        let bad_row = recent
            .iter()
            .find(|row| row.id == bad_run)
            .expect("bad row");
        assert_eq!(bad_row.ok, Some(false));
    }

    #[test]
    fn interrupted_outcome_is_a_failure() {
        let outcome = interrupted_outcome();
        assert!(!outcome.ok);
        assert!(outcome.message.contains("interrupted"));
        assert_eq!(outcome.redirect, None);
    }

    #[tokio::test]
    async fn acks_are_per_user_and_durable() {
        let (repo, _dir) = repo_with(300, 600).await;
        let run = repo.insert("alpha").await.expect("insert");

        assert!(repo.acked_run_ids(1).await.expect("acks").is_empty());
        repo.ack(&run, 1).await.expect("ack");
        repo.ack(&run, 1).await.expect("ack again");
        assert_eq!(
            repo.acked_run_ids(1).await.expect("acks"),
            vec![run.clone()]
        );
        assert!(
            repo.acked_run_ids(2).await.expect("other user").is_empty(),
            "acks are scoped per user"
        );
    }

    #[tokio::test]
    async fn list_for_actor_filters_by_user_id() {
        let (repo, _dir) = repo_with(300, 600).await;
        repo.insert_with(&NewRun {
            subject: "alpha".to_owned(),
            operation: "app.restart".to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: Some(1),
                email: Some("ops@example.com".to_owned()),
            },
            parent_run_id: None,
        })
        .await
        .expect("user run");
        repo.insert_with(&NewRun {
            subject: "beta".to_owned(),
            operation: "app.stop".to_owned(),
            target_kind: TargetKind::App,
            actor: Actor {
                user_id: Some(2),
                email: Some("other@example.com".to_owned()),
            },
            parent_run_id: None,
        })
        .await
        .expect("other run");

        let rows = repo.list_for_actor(1, 10).await.expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].operation, "app.restart");
    }

    #[test]
    fn target_kind_roundtrips() {
        for kind in [
            TargetKind::App,
            TargetKind::Service,
            TargetKind::Volume,
            TargetKind::System,
        ] {
            assert_eq!(TargetKind::try_from(kind.as_str()), Some(kind));
        }
        assert_eq!(TargetKind::try_from("nope"), None);
    }

    #[test]
    fn format_when_buckets() {
        let now = now_epoch();
        assert_eq!(format_when(now), "just now");
        assert_eq!(format_when(now - 30), "just now");
        assert_eq!(format_when(now - 300), "5m ago");
        assert_eq!(format_when(now - 3600 * 3), "3h ago");
        assert_eq!(format_when(now - 86_400 * 2), "2d ago");
        let old = format_when(now - 86_400 * 400);
        assert!(old.contains('-'), "older than a week shows a date: {old}");
        assert_eq!(
            format_when(now + 1000),
            "just now",
            "future timestamps clamp"
        );
    }

    #[test]
    fn summary_labels() {
        let summary = |ok: Option<bool>| -> RunSummary {
            RunSummary {
                id: "x".into(),
                operation: "app.restart".into(),
                target_kind: TargetKind::App,
                subject: "alpha".into(),
                actor_email: Some("ops@example.com".into()),
                ok,
                created_at: 0,
            }
        };
        assert_eq!(summary(Some(true)).outcome_label(), "succeeded");
        assert_eq!(summary(Some(false)).outcome_label(), "failed");
        assert_eq!(summary(None).outcome_label(), "running");
        assert_eq!(summary(None).actor_email_label(), "ops@example.com");
        let system = summary(None);
        let system = RunSummary {
            actor_email: None,
            ..system
        };
        assert_eq!(system.actor_email_label(), "system");
    }
}
