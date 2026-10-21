use sqlx::SqlitePool;

/// Last custom-env string the UI set for a service. The installed plugin
/// generation cannot report custom env back, and `<plugin>:upgrade` clears it
/// unless resubmitted, so the UI records what it applied to prefill the
/// upgrade form. CLI-created services have no row (unknown env).
#[derive(Debug, Clone)]
pub struct SqliteServiceImageConfig {
    pool: SqlitePool,
}

impl SqliteServiceImageConfig {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, plugin: &str, service: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT custom_env FROM service_image_config WHERE plugin = ? AND service = ?",
        )
        .bind(plugin)
        .bind(service)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn put(
        &self,
        plugin: &str,
        service: &str,
        custom_env: &str,
    ) -> Result<(), sqlx::Error> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        sqlx::query(
            "INSERT INTO service_image_config (plugin, service, custom_env, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(plugin, service) DO UPDATE SET custom_env = excluded.custom_env, \
             updated_at = excluded.updated_at",
        )
        .bind(plugin)
        .bind(service)
        .bind(custom_env)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, plugin: &str, service: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM service_image_config WHERE plugin = ? AND service = ?")
            .bind(plugin)
            .bind(service)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::storage;

    async fn repo() -> (SqliteServiceImageConfig, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/cfg.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteServiceImageConfig::new(pool), dir)
    }

    #[tokio::test]
    async fn missing_row_is_none() {
        let (repo, _dir) = repo().await;
        assert_eq!(repo.get("redis", "candid").await.expect("get"), None);
    }

    #[tokio::test]
    async fn put_get_upsert_and_delete() {
        let (repo, _dir) = repo().await;
        repo.put("redis", "candid", "USER=alpha")
            .await
            .expect("put");
        assert_eq!(
            repo.get("redis", "candid").await.expect("get").as_deref(),
            Some("USER=alpha")
        );

        repo.put("redis", "candid", "").await.expect("upsert empty");
        assert_eq!(
            repo.get("redis", "candid").await.expect("get").as_deref(),
            Some("")
        );

        repo.delete("redis", "candid").await.expect("delete");
        assert_eq!(repo.get("redis", "candid").await.expect("get"), None);
    }
}
