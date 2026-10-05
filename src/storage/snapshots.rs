use sqlx::SqlitePool;

/// The singleton snapshot row: the full JSON payload, the epoch second the
/// data was fetched at (drives the displayed age), and the epoch millisecond
/// the row was last modified. `updated_at` exists only so a full refresh can
/// tell whether a per-app patch from another process landed while its SSH
/// build was running — such a patch must never be clobbered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSnapshot {
    pub data: String,
    pub fetched_at: i64,
    pub updated_at: i64,
}

impl StoredSnapshot {
    /// A payload awaiting a write path; the write methods stamp `updated_at`.
    pub fn new(data: String, fetched_at: i64) -> Self {
        Self {
            data,
            fetched_at,
            updated_at: 0,
        }
    }
}

/// Epoch milliseconds — the granularity at which "was this row modified
/// after that build started" is decided.
pub fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

/// Row I/O for the shared snapshot table. All containers read and write the
/// same row, so a refresh on one is immediately visible to every other.
#[derive(Debug, Clone)]
pub struct SqliteSnapshots {
    pool: SqlitePool,
}

impl SqliteSnapshots {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn load(&self) -> Result<Option<StoredSnapshot>, sqlx::Error> {
        load(&self.pool).await
    }

    /// Publishes a full snapshot, unless the row was modified after
    /// `since_ms` — a per-app patch from another process landed while this
    /// build ran and must not be overwritten with staler data. The whole
    /// check-then-write runs under `BEGIN IMMEDIATE`, so a patch cannot slip
    /// between the check and the save. Returns `false` when the newer row was
    /// kept.
    pub async fn save_if_unmodified_since(
        &self,
        stored: &StoredSnapshot,
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

    /// Read-modify-write of the singleton row under a `BEGIN IMMEDIATE`
    /// transaction, so concurrent per-app patches from different processes
    /// serialize instead of losing updates. `f` must be fast (no I/O): the
    /// write lock is held for its whole duration. A patch to an existing row
    /// stamps `updated_at` (so in-flight full refreshes yield to it); a seed
    /// from an absent row stays `0` — a seed is incomplete by definition, and
    /// any full build may overwrite it.
    pub async fn patch<F, T>(&self, f: F) -> Result<T, sqlx::Error>
    where
        F: FnOnce(Option<StoredSnapshot>) -> Result<(StoredSnapshot, T), sqlx::Error>,
    {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let result = async {
            let current = load(&mut *conn).await?;
            let row_existed = current.is_some();
            let (mut next, value) = f(current)?;
            next.updated_at = if row_existed { now_ms() } else { 0 };
            save(&mut *conn, &next).await?;
            Ok(value)
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

async fn load<'e, E>(executor: E) -> Result<Option<StoredSnapshot>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row: Option<(String, i64, i64)> =
        sqlx::query_as("SELECT data, fetched_at, updated_at FROM snapshots WHERE id = 1")
            .fetch_optional(executor)
            .await?;
    Ok(row.map(|(data, fetched_at, updated_at)| StoredSnapshot {
        data,
        fetched_at,
        updated_at,
    }))
}

async fn save<'e, E>(executor: E, stored: &StoredSnapshot) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO snapshots (id, data, fetched_at, updated_at) VALUES (1, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET data = excluded.data, fetched_at = excluded.fetched_at, \
         updated_at = excluded.updated_at",
    )
    .bind(&stored.data)
    .bind(stored.fetched_at)
    .bind(stored.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    async fn store() -> (SqliteSnapshots, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/snapshots.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteSnapshots::new(pool), dir)
    }

    fn stored(data: &str) -> StoredSnapshot {
        StoredSnapshot::new(data.to_owned(), 1_700_000_000)
    }

    async fn forge_updated_at(store: &SqliteSnapshots, updated_at: i64) {
        sqlx::query("UPDATE snapshots SET updated_at = ? WHERE id = 1")
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
    async fn publish_when_absent_roundtrips_data_and_fetch_time() {
        let (store, _dir) = store().await;
        let value = stored(r#"{"apps":[]}"#);
        let saved = store
            .save_if_unmodified_since(&value, 0)
            .await
            .expect("save");
        assert!(saved, "absent row is published");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, value.data);
        assert_eq!(loaded.fetched_at, value.fetched_at);
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
        assert!(saved, "row was not modified after the build started");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, "second");
    }

    #[tokio::test]
    async fn publish_yields_to_a_row_modified_after_the_build_started() {
        let (store, _dir) = store().await;
        assert!(
            store
                .save_if_unmodified_since(&stored("patched"), 0)
                .await
                .expect("seed")
        );
        let older = store.load().await.expect("load").expect("row").updated_at;
        // A patch that lands "now": newer than any build that started earlier.
        forge_updated_at(&store, older + 60_000).await;

        let saved = store
            .save_if_unmodified_since(&stored("stale full pass"), older + 30_000)
            .await
            .expect("conditional save");
        assert!(!saved, "the newer patch is kept");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, "patched");
    }

    #[tokio::test]
    async fn patch_read_modify_writes_the_row() {
        let (store, _dir) = store().await;
        assert!(
            store
                .save_if_unmodified_since(&stored(r#"{"apps":["alpha"]}"#), 0)
                .await
                .expect("seed")
        );
        let value = store
            .patch(|current| {
                let mut data = current.expect("row").data;
                data.push_str("+patched");
                Ok((
                    StoredSnapshot {
                        data,
                        fetched_at: 9,
                        updated_at: 0,
                    },
                    "returned",
                ))
            })
            .await
            .expect("patch");
        assert_eq!(value, "returned");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, r#"{"apps":["alpha"]}+patched"#);
        assert_eq!(loaded.fetched_at, 9);
        assert!(loaded.updated_at > 0, "patching an existing row stamps it");
    }

    #[tokio::test]
    async fn patch_with_no_row_seeds_without_stamp() {
        let (store, _dir) = store().await;
        store
            .patch(|current| {
                assert!(current.is_none());
                Ok((stored("seeded"), ()))
            })
            .await
            .expect("patch");
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(loaded.data, "seeded");
        assert_eq!(
            loaded.updated_at, 0,
            "a seed is incomplete; any in-flight full build may overwrite it"
        );
    }

    #[tokio::test]
    async fn patch_rolls_back_when_closure_fails() {
        let (store, _dir) = store().await;
        assert!(
            store
                .save_if_unmodified_since(&stored("original"), 0)
                .await
                .expect("seed")
        );
        let err = store
            .patch::<_, ()>(|_current| Err(sqlx::Error::PoolClosed))
            .await
            .expect_err("closure fails");
        assert!(matches!(err, sqlx::Error::PoolClosed));
        let loaded = store.load().await.expect("load").expect("row");
        assert_eq!(
            loaded.data, "original",
            "failed patch leaves the row untouched"
        );
    }
}
