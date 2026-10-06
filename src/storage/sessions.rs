use time::Duration;

use actix_session::storage::{LoadError, SaveError, SessionKey, SessionStore, UpdateError};
use sqlx::SqlitePool;
use std::collections::HashMap;
use time::OffsetDateTime;

use crate::auth::csrf::random_session_key;

#[derive(Debug, Clone)]
pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Deletes every session whose stored state belongs to `user_id`,
    /// returning how many were removed. Password changes and admin resets use
    /// this to revoke a possibly-copied cookie; sessions without a
    /// `user_id` key (pre-login) are never touched.
    pub async fn delete_user_sessions(&self, user_id: i64) -> Result<u64, sqlx::Error> {
        let deleted = sqlx::query(
            "DELETE FROM sessions WHERE CAST(json_extract(data, '$.user_id') AS TEXT) = ?",
        )
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(deleted)
    }
}

fn now_epoch() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

impl SessionStore for SqliteSessionStore {
    async fn load(
        &self,
        session_key: &SessionKey,
    ) -> Result<Option<HashMap<String, String>>, LoadError> {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT data, expires_at FROM sessions WHERE id = ?")
                .bind(session_key.as_ref())
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| LoadError::Other(anyhow::Error::from(err)))?;

        let Some((data, expires_at)) = row else {
            return Ok(None);
        };
        if expires_at < now_epoch() {
            let _ = self.delete(session_key).await;
            return Ok(None);
        }
        serde_json::from_str(&data)
            .map(Some)
            .map_err(|err| LoadError::Deserialization(anyhow::Error::from(err)))
    }

    async fn save(
        &self,
        session_state: HashMap<String, String>,
        ttl: &Duration,
    ) -> Result<SessionKey, SaveError> {
        let key = SessionKey::try_from(random_session_key())
            .map_err(|err| SaveError::Other(anyhow::Error::from(err)))?;
        let data = serde_json::to_string(&session_state)
            .map_err(|err| SaveError::Serialization(anyhow::Error::from(err)))?;
        let expires_at = now_epoch() + ttl.whole_seconds();
        sqlx::query("INSERT INTO sessions (id, data, expires_at) VALUES (?, ?, ?)")
            .bind(key.as_ref())
            .bind(&data)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(|err| SaveError::Other(anyhow::Error::from(err)))?;
        Ok(key)
    }

    async fn update(
        &self,
        session_key: SessionKey,
        session_state: HashMap<String, String>,
        ttl: &Duration,
    ) -> Result<SessionKey, UpdateError> {
        let data = serde_json::to_string(&session_state)
            .map_err(|err| UpdateError::Serialization(anyhow::Error::from(err)))?;
        let expires_at = now_epoch() + ttl.whole_seconds();
        sqlx::query("UPDATE sessions SET data = ?, expires_at = ? WHERE id = ?")
            .bind(&data)
            .bind(expires_at)
            .bind(session_key.as_ref())
            .execute(&self.pool)
            .await
            .map_err(|err| UpdateError::Other(anyhow::Error::from(err)))?;
        Ok(session_key)
    }

    async fn update_ttl(
        &self,
        session_key: &SessionKey,
        ttl: &Duration,
    ) -> Result<(), anyhow::Error> {
        let expires_at = now_epoch() + ttl.whole_seconds();
        sqlx::query("UPDATE sessions SET expires_at = ? WHERE id = ?")
            .bind(expires_at)
            .bind(session_key.as_ref())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete(&self, session_key: &SessionKey) -> Result<(), anyhow::Error> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_key.as_ref())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::TempDir;

    use super::*;
    use crate::storage;

    async fn store() -> (SqliteSessionStore, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/sessions.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteSessionStore::new(pool), dir)
    }

    fn state(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn ttl() -> Duration {
        Duration::seconds(3600)
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let (store, _dir) = store().await;
        let key = store
            .save(state(&[("user_id", "42")]), &ttl())
            .await
            .expect("save");
        let loaded = store.load(&key).await.expect("load").expect("some state");
        assert_eq!(loaded.get("user_id").map(String::as_str), Some("42"));
    }

    #[tokio::test]
    async fn load_unknown_key_returns_none() {
        let (store, _dir) = store().await;
        let key = SessionKey::try_from("unknown-key".to_owned()).expect("key");
        assert!(store.load(&key).await.expect("load").is_none());
    }

    #[tokio::test]
    async fn load_expired_session_returns_none_and_purges() {
        let (store, _dir) = store().await;
        let key = SessionKey::try_from("expired-key".to_owned()).expect("key");
        sqlx::query("INSERT INTO sessions (id, data, expires_at) VALUES (?, ?, ?)")
            .bind(key.as_ref())
            .bind("{}")
            .bind(now_epoch() - 10)
            .execute(&store.pool)
            .await
            .expect("insert expired row");

        assert!(store.load(&key).await.expect("load").is_none());
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id = ?")
            .bind(key.as_ref())
            .fetch_one(&store.pool)
            .await
            .expect("count");
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn update_overwrites_state_and_returns_key() {
        let (store, _dir) = store().await;
        let key = store.save(state(&[]), &ttl()).await.expect("save");
        let key_string = key.as_ref().to_owned();
        let updated = store
            .update(key, state(&[("user_id", "7")]), &ttl())
            .await
            .expect("update");
        assert_eq!(updated.as_ref(), key_string);
        let loaded_key = SessionKey::try_from(key_string).expect("key");
        let loaded = store.load(&loaded_key).await.expect("load").expect("some");
        assert_eq!(loaded.get("user_id").map(String::as_str), Some("7"));
    }

    #[tokio::test]
    async fn update_ttl_extends_expiry() {
        let (store, _dir) = store().await;
        let key = store.save(state(&[]), &ttl()).await.expect("save");
        store
            .update_ttl(&key, &Duration::seconds(86_400))
            .await
            .expect("update ttl");
        let (_, expires_at): (String, i64) =
            sqlx::query_as("SELECT data, expires_at FROM sessions WHERE id = ?")
                .bind(key.as_ref())
                .fetch_one(&store.pool)
                .await
                .expect("row");
        assert!(expires_at > now_epoch() + 80_000);
    }

    #[tokio::test]
    async fn delete_removes_session() {
        let (store, _dir) = store().await;
        let key = store.save(state(&[]), &ttl()).await.expect("save");
        store.delete(&key).await.expect("delete");
        assert!(store.load(&key).await.expect("load").is_none());
    }

    #[tokio::test]
    async fn state_json_is_stored_raw() {
        let (store, _dir) = store().await;
        let key = store
            .save(state(&[("a", "1"), ("b", "2")]), &ttl())
            .await
            .expect("save");
        let row: (String,) = sqlx::query_as("SELECT data FROM sessions WHERE id = ?")
            .bind(key.as_ref())
            .fetch_one(&store.pool)
            .await
            .expect("row");
        let parsed: HashMap<String, String> = serde_json::from_str(&row.0).expect("json");
        assert_eq!(parsed.len(), 2);
    }

    #[tokio::test]
    async fn delete_user_sessions_removes_only_that_user() {
        let (store, _dir) = store().await;
        let first = store
            .save(state(&[("user_id", "7")]), &ttl())
            .await
            .expect("save");
        let second = store
            .save(state(&[("user_id", "7")]), &ttl())
            .await
            .expect("save");
        let other = store
            .save(state(&[("user_id", "8")]), &ttl())
            .await
            .expect("save");
        let anonymous = store
            .save(state(&[("csrf_token", "x")]), &ttl())
            .await
            .expect("save");

        assert_eq!(store.delete_user_sessions(7).await.expect("purge"), 2);
        assert!(store.load(&first).await.expect("load").is_none());
        assert!(store.load(&second).await.expect("load").is_none());
        assert!(store.load(&other).await.expect("load").is_some());
        assert!(store.load(&anonymous).await.expect("load").is_some());
    }
}
