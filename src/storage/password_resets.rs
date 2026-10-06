use sqlx::SqlitePool;

#[async_trait::async_trait]
pub trait PasswordResetsRepo: Send + Sync {
    /// Stores a new reset token hash for the user, invalidating any outstanding
    /// unused links first.
    async fn create(
        &self,
        user_id: i64,
        token_hash: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<(), sqlx::Error>;
    /// Atomically marks a live token used and returns its user id; `None` for
    /// unknown, expired, or already-used tokens.
    async fn consume(&self, token_hash: &str, now: i64) -> Result<Option<i64>, sqlx::Error>;
}

#[derive(Debug, Clone)]
pub struct SqlitePasswordResetsRepo {
    pool: SqlitePool,
}

impl SqlitePasswordResetsRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl PasswordResetsRepo for SqlitePasswordResetsRepo {
    async fn create(
        &self,
        user_id: i64,
        token_hash: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM password_resets WHERE user_id = ? AND used_at IS NULL")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM password_resets WHERE used_at IS NOT NULL OR expires_at < ?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO password_resets (token_hash, user_id, created_at, expires_at, used_at) \
             VALUES (?, ?, ?, ?, NULL)",
        )
        .bind(token_hash)
        .bind(user_id)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn consume(&self, token_hash: &str, now: i64) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "UPDATE password_resets SET used_at = ? \
             WHERE token_hash = ? AND used_at IS NULL AND expires_at > ? \
             RETURNING user_id",
        )
        .bind(now)
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::storage;

    async fn repo() -> (SqlitePasswordResetsRepo, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/resets.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqlitePasswordResetsRepo::new(pool), dir)
    }

    #[tokio::test]
    async fn consume_is_single_use() {
        let (repo, _dir) = repo().await;
        repo.create(7, "hash-a", 1000, 2000).await.expect("create");
        assert_eq!(
            repo.consume("hash-a", 1500).await.expect("consume"),
            Some(7)
        );
        assert_eq!(repo.consume("hash-a", 1500).await.expect("consume"), None);
    }

    #[tokio::test]
    async fn expired_and_unknown_tokens_are_rejected() {
        let (repo, _dir) = repo().await;
        repo.create(7, "hash-a", 1000, 2000).await.expect("create");
        assert_eq!(repo.consume("hash-a", 2000).await.expect("consume"), None);
        assert_eq!(repo.consume("other", 1500).await.expect("consume"), None);
    }

    #[tokio::test]
    async fn create_invalidates_previous_unused_links() {
        let (repo, _dir) = repo().await;
        repo.create(7, "old", 1000, 2000).await.expect("create");
        repo.create(7, "new", 1100, 2100).await.expect("create");
        assert_eq!(repo.consume("old", 1200).await.expect("consume"), None);
        assert_eq!(repo.consume("new", 1200).await.expect("consume"), Some(7));
    }

    #[tokio::test]
    async fn create_prunes_finished_and_expired_rows() {
        let (repo, _dir) = repo().await;
        repo.create(1, "used", 1000, 5000).await.expect("create");
        repo.consume("used", 1100).await.expect("consume");
        repo.create(2, "expired", 1000, 1500).await.expect("create");
        repo.create(3, "live", 1600, 9000).await.expect("create");

        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT token_hash FROM password_resets ORDER BY token_hash")
                .fetch_all(&repo.pool)
                .await
                .expect("rows");
        let names: Vec<String> = rows.into_iter().map(|(name,)| name).collect();
        assert_eq!(names, vec!["live".to_owned()]);
    }
}
