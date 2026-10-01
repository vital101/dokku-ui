use std::io;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

pub mod sessions;
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
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
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
}
