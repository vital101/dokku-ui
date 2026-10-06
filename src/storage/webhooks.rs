use sqlx::SqlitePool;

use super::snapshots::now_ms;

/// A per-app GitHub webhook configuration. `secret` is the HMAC key GitHub
/// signs deliveries with; it is revealed only under re-auth and masked from
/// every run line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Webhook {
    pub app: String,
    pub repo: String,
    pub branch: String,
    /// `build`, `build-if-changes`, or `no-build` (see `GitBuildMode`).
    pub build_mode: String,
    pub secret: String,
    pub enabled: bool,
}

/// Row I/O for the shared `app_webhooks` table, so every container sees the
/// same configuration.
#[derive(Debug, Clone)]
pub struct SqliteWebhooksRepo {
    pool: SqlitePool,
}

impl SqliteWebhooksRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, app: &str) -> Result<Option<Webhook>, sqlx::Error> {
        sqlx::query_as::<_, WebhookRow>(
            "SELECT app, repo, branch, build_mode, secret, enabled FROM app_webhooks WHERE app = ?",
        )
        .bind(app)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(Webhook::from))
    }

    /// Creates or replaces the app's webhook configuration.
    pub async fn upsert(&self, webhook: &Webhook) -> Result<(), sqlx::Error> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO app_webhooks (app, repo, branch, build_mode, secret, enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(app) DO UPDATE SET
               repo = excluded.repo,
               branch = excluded.branch,
               build_mode = excluded.build_mode,
               secret = excluded.secret,
               enabled = excluded.enabled,
               updated_at = excluded.updated_at",
        )
        .bind(&webhook.app)
        .bind(&webhook.repo)
        .bind(&webhook.branch)
        .bind(&webhook.build_mode)
        .bind(&webhook.secret)
        .bind(webhook.enabled)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    pub async fn delete(&self, app: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM app_webhooks WHERE app = ?")
            .bind(app)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }
}

#[derive(sqlx::FromRow)]
struct WebhookRow {
    app: String,
    repo: String,
    branch: String,
    build_mode: String,
    secret: String,
    enabled: bool,
}

impl From<WebhookRow> for Webhook {
    fn from(row: WebhookRow) -> Self {
        Self {
            app: row.app,
            repo: row.repo,
            branch: row.branch,
            build_mode: row.build_mode,
            secret: row.secret,
            enabled: row.enabled,
        }
    }
}
