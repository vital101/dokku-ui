use sqlx::SqlitePool;

use super::snapshots::now_ms;

/// The singleton capabilities row: the full JSON payload plus the epoch
/// millisecond the row was last modified, so an in-flight probe from another
/// process can tell whether a newer row landed while its SSH probe ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCapabilities {
    pub data: String,
    pub updated_at: i64,
}

impl StoredCapabilities {
    /// A payload awaiting a write path; the write path stamps `updated_at`.
    pub fn new(data: String) -> Self {
        Self {
            data,
            updated_at: 0,
        }
    }
}

/// Row I/O for the shared capabilities table. All containers read and write
/// the same row, so a probe on one is immediately visible to every other.
#[derive(Debug, Clone)]
pub struct SqliteCapabilities {
    pool: SqlitePool,
}

impl SqliteCapabilities {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn load(&self) -> Result<Option<StoredCapabilities>, sqlx::Error> {
        load(&self.pool).await
    }

    /// Publishes a fresh probe, unless the row was modified after `since_ms` —
    /// another process published while this probe ran and must not be
    /// overwritten with staler data. The whole check-then-write runs under
    /// `BEGIN IMMEDIATE`, so a save cannot slip between the check and the
    /// write. Returns `false` when the newer row was kept.
    pub async fn save_if_unmodified_since(
        &self,
        stored: &StoredCapabilities,
        since_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let result = async {
            let current = load(&mut *conn).await?;
            if current
                .as_ref()
                .is_some_and(|row| row.updated_at > since_ms)
            {
                return Ok(false);
            }
            let mut next = stored.clone();
            next.updated_at = now_ms();
            save(&mut *conn, &next).await?;
            Ok(true)
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
}

async fn load<'e, E>(executor: E) -> Result<Option<StoredCapabilities>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT data, updated_at FROM capabilities WHERE id = 1")
            .fetch_optional(executor)
            .await?;
    Ok(row.map(|(data, updated_at)| StoredCapabilities { data, updated_at }))
}

async fn save<'e, E>(executor: E, stored: &StoredCapabilities) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO capabilities (id, data, updated_at) VALUES (1, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
    )
    .bind(&stored.data)
    .bind(stored.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    async fn store() -> (SqliteCapabilities, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/capabilities.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteCapabilities::new(pool), dir)
    }

    fn stored(data: &str) -> StoredCapabilities {
        StoredCapabilities::new(data.to_owned())
    }

    async fn forge_updated_at(store: &SqliteCapabilities, updated_at: i64) {
        sqlx::query("UPDATE capabilities SET updated_at = ? WHERE id = 1")
            .bind(updated_at)
            .execute(&store.pool)
            .await
            .expect("forge updated_at");
    }

    #[tokio::test]
    async fn load_returns_none_when_empty() {
        let (store, _dir) = store().await;
        assert!(store.load().await.expect("load").is_none());
    }

    #[tokio::test]
    async fn publish_when_absent_roundtrips_data() {
        let (store, _dir) = store().await;
        let saved = store
            .save_if_unmodified_since(&stored("probed"), 0)
            .await
            .expect("save");
        assert!(saved, "absent row is published");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, "probed");
        assert!(loaded.updated_at > 0, "the write path stamps updated_at");
    }

    #[tokio::test]
    async fn a_second_publish_replaces_an_older_row() {
        let (store, _dir) = store().await;
        assert!(
            store
                .save_if_unmodified_since(&stored("first"), 0)
                .await
                .expect("first")
        );
        let older = store.load().await.expect("load").expect("row").updated_at;
        let saved = store
            .save_if_unmodified_since(&stored("second"), older)
            .await
            .expect("second");
        assert!(saved, "row was not modified after the probe started");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, "second");
    }

    #[tokio::test]
    async fn publish_yields_to_a_row_modified_after_the_probe_started() {
        let (store, _dir) = store().await;
        assert!(
            store
                .save_if_unmodified_since(&stored("probed"), 0)
                .await
                .expect("seed")
        );
        let older = store.load().await.expect("load").expect("row").updated_at;
        forge_updated_at(&store, older + 60_000).await;

        let saved = store
            .save_if_unmodified_since(&stored("stale probe"), older + 30_000)
            .await
            .expect("conditional save");
        assert!(!saved, "the newer row is kept");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, "probed");
    }
}
