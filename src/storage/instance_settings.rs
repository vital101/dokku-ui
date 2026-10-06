use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::domain::InstanceSettings;

#[async_trait::async_trait]
pub trait InstanceSettingsRepo: Send + Sync {
    async fn load(&self) -> Result<InstanceSettings, sqlx::Error>;
    async fn save(&self, settings: &InstanceSettings) -> Result<(), sqlx::Error>;
}

#[derive(Debug, Clone)]
pub struct SqliteInstanceSettingsRepo {
    pool: SqlitePool,
}

impl SqliteInstanceSettingsRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl InstanceSettingsRepo for SqliteInstanceSettingsRepo {
    async fn load(&self) -> Result<InstanceSettings, sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM instance_settings")
                .fetch_all(&self.pool)
                .await?;
        let map: HashMap<String, String> = rows.into_iter().collect();
        Ok(InstanceSettings::from_map(&map))
    }

    async fn save(&self, settings: &InstanceSettings) -> Result<(), sqlx::Error> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM instance_settings")
            .execute(&mut *tx)
            .await?;
        for (key, value) in settings.to_map() {
            sqlx::query("INSERT INTO instance_settings (key, value, updated_at) VALUES (?, ?, ?)")
                .bind(key)
                .bind(value)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::storage;

    async fn repo() -> (SqliteInstanceSettingsRepo, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/settings.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteInstanceSettingsRepo::new(pool), dir)
    }

    #[tokio::test]
    async fn load_defaults_to_empty() {
        let (repo, _dir) = repo().await;
        assert_eq!(
            repo.load().await.expect("load"),
            InstanceSettings::default()
        );
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (repo, _dir) = repo().await;
        let settings = InstanceSettings::parse(
            Some("https://ui.example.com"),
            Some("Hello"),
            Some("dokku-*"),
            Some("86400"),
            Some("3600"),
        )
        .expect("valid");
        repo.save(&settings).await.expect("save");
        assert_eq!(repo.load().await.expect("load"), settings);
    }

    #[tokio::test]
    async fn saving_blanks_clears_stored_values() {
        let (repo, _dir) = repo().await;
        repo.save(
            &InstanceSettings::parse(Some("https://ui.example.com"), Some("x"), None, None, None)
                .expect("valid"),
        )
        .await
        .expect("save");
        repo.save(&InstanceSettings::default())
            .await
            .expect("save blank");
        assert_eq!(
            repo.load().await.expect("load"),
            InstanceSettings::default()
        );
    }
}
