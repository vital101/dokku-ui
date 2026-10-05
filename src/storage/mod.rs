use std::future::Future;
use std::io;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

pub mod runs;
pub mod sessions;
pub mod snapshots;
pub mod users;

pub fn ensure_db_parent_dir(database_url: &str) -> io::Result<()> {
    let Some(path) = database_url.strip_prefix("sqlite://") else {
        return Ok(());
    };
    let path = path.split('?').next().unwrap_or(path);
    if path == ":memory:" {
        return Ok(());
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

pub async fn connect(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

/// Applies migrations with retries: on a fresh deploy every container runs
/// migrations at boot against the same file, and exactly one of them must win
/// the DDL race. Retrying lets the losers pick up the winner's schema instead
/// of crash-looping once on a `table already exists` error.
async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    const MIGRATION_ATTEMPTS: usize = 5;
    const MIGRATION_BASE_DELAY: Duration = Duration::from_millis(500);
    with_retries(MIGRATION_ATTEMPTS, MIGRATION_BASE_DELAY, || async {
        sqlx::migrate!().run(pool).await
    })
    .await
}

/// Runs `f` up to `attempts` times, sleeping an exponentially growing delay
/// between failures. The final attempt's error is returned as-is.
async fn with_retries<F, Fut, T, E>(attempts: usize, base_delay: Duration, mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut delay = base_delay;
    for attempt in 1..attempts {
        match f().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                tracing::warn!(attempt, error = %err, "operation failed; retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
    }
    f().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_parent_dir_for_sqlite_file_urls() {
        let dir = tempfile::tempdir().expect("temp dir");
        let url = format!("sqlite://{}/nested/db.sqlite", dir.path().display());
        ensure_db_parent_dir(&url).expect("create dir");
        assert!(dir.path().join("nested").is_dir());
    }

    #[test]
    fn ignores_non_sqlite_urls() {
        assert!(ensure_db_parent_dir("postgres://user:pass@host/db").is_ok());
    }

    #[test]
    fn ignores_in_memory_dbs() {
        assert!(ensure_db_parent_dir("sqlite::memory:").is_ok());
        assert!(ensure_db_parent_dir("sqlite://:memory:").is_ok());
    }

    #[tokio::test]
    async fn connect_runs_migrations() {
        let dir = tempfile::tempdir().expect("temp dir");
        let url = format!("sqlite://{}/db.sqlite", dir.path().display());
        let pool = connect(&url).await.expect("connect");
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('users', 'sessions')",
        )
        .fetch_all(&pool)
        .await
        .expect("query tables");
        assert_eq!(tables.len(), 2);
    }

    #[tokio::test]
    async fn connect_runs_new_migrations() {
        let dir = tempfile::tempdir().expect("temp dir");
        let url = format!("sqlite://{}/db.sqlite", dir.path().display());
        let pool = connect(&url).await.expect("connect");
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name IN ('snapshots', 'action_runs', 'action_run_lines')",
        )
        .fetch_all(&pool)
        .await
        .expect("query tables");
        assert_eq!(tables.len(), 3);
    }

    #[tokio::test]
    async fn with_retries_succeeds_on_the_first_attempt() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let result = with_retries(5, Duration::from_millis(1), move || {
            let calls = calls_clone.clone();
            async move {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, &str>("ok")
            }
        })
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn with_retries_keeps_trying_until_success() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let result = with_retries(5, Duration::from_millis(1), move || {
            let calls = calls_clone.clone();
            async move {
                let attempt = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if attempt < 2 {
                    Err("boom")
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;
        assert_eq!(result, Ok(2));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn with_retries_gives_up_after_the_attempt_budget() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let result = with_retries(3, Duration::from_millis(1), move || {
            let calls = calls_clone.clone();
            async move {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err::<(), _>("always fails")
            }
        })
        .await;
        assert_eq!(result, Err("always fails"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
