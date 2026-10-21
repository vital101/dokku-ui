use sqlx::SqlitePool;

/// A cache row: the tag list, when it was fetched, and when the last refresh
/// failed (0 when the last attempt succeeded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTags {
    pub tags: Vec<String>,
    pub fetched_at: i64,
    pub failed_at: i64,
}

/// Shared, durable cache of Docker Hub tag lists, keyed by repository. All
/// containers read and write the same table, so the tag list survives restarts
/// and only refreshes when stale.
#[derive(Debug, Clone)]
pub struct SqliteImageTagCache {
    pool: SqlitePool,
}

impl SqliteImageTagCache {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Returns the cached row, if any.
    pub async fn get(&self, repo: &str) -> Result<Option<CachedTags>, sqlx::Error> {
        let row: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT tags_json, fetched_at, failed_at FROM image_tag_cache WHERE repo = ?",
        )
        .bind(repo)
        .fetch_optional(&self.pool)
        .await?;
        let Some((tags_json, fetched_at, failed_at)) = row else {
            return Ok(None);
        };
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        Ok(Some(CachedTags {
            tags,
            fetched_at,
            failed_at,
        }))
    }

    pub async fn put(
        &self,
        repo: &str,
        tags: &[String],
        fetched_at: i64,
    ) -> Result<(), sqlx::Error> {
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_owned());
        sqlx::query(
            "INSERT INTO image_tag_cache (repo, tags_json, fetched_at, failed_at) \
             VALUES (?, ?, ?, 0) \
             ON CONFLICT(repo) DO UPDATE SET tags_json = excluded.tags_json, \
             fetched_at = excluded.fetched_at, failed_at = 0",
        )
        .bind(repo)
        .bind(tags_json)
        .bind(fetched_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Records a failed refresh without discarding the last good tag list.
    pub async fn mark_failure(&self, repo: &str, failed_at: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO image_tag_cache (repo, tags_json, fetched_at, failed_at) \
             VALUES (?, '[]', 0, ?) \
             ON CONFLICT(repo) DO UPDATE SET failed_at = excluded.failed_at",
        )
        .bind(repo)
        .bind(failed_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[cfg(test)]
    pub async fn failed_at(&self, repo: &str) -> i64 {
        self.get(repo)
            .await
            .expect("get")
            .map(|cached| cached.failed_at)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::storage;

    async fn cache() -> (SqliteImageTagCache, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/tags.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteImageTagCache::new(pool), dir)
    }

    #[tokio::test]
    async fn missing_repo_is_none() {
        let (cache, _dir) = cache().await;
        assert_eq!(cache.get("library/redis").await.expect("get"), None);
    }

    #[tokio::test]
    async fn put_then_get_roundtrips_and_upserts() {
        let (cache, _dir) = cache().await;
        cache
            .put("library/redis", &["7.2.4".to_owned()], 100)
            .await
            .expect("put");
        assert_eq!(
            cache.get("library/redis").await.expect("get"),
            Some(CachedTags {
                tags: vec!["7.2.4".to_owned()],
                fetched_at: 100,
                failed_at: 0,
            })
        );

        cache
            .put(
                "library/redis",
                &["8.0.1".to_owned(), "8.0.2".to_owned()],
                200,
            )
            .await
            .expect("upsert");
        assert_eq!(
            cache.get("library/redis").await.expect("get"),
            Some(CachedTags {
                tags: vec!["8.0.1".to_owned(), "8.0.2".to_owned()],
                fetched_at: 200,
                failed_at: 0,
            })
        );
    }

    #[tokio::test]
    async fn mark_failure_keeps_the_last_good_tags_and_clears_on_success() {
        let (cache, _dir) = cache().await;
        cache
            .put("library/redis", &["7.2.4".to_owned()], 100)
            .await
            .expect("put");
        cache
            .mark_failure("library/redis", 150)
            .await
            .expect("mark");
        assert_eq!(cache.failed_at("library/redis").await, 150);
        assert_eq!(
            cache
                .get("library/redis")
                .await
                .expect("get")
                .expect("row")
                .tags,
            vec!["7.2.4".to_owned()]
        );

        cache
            .put("library/redis", &["8.0.1".to_owned()], 200)
            .await
            .expect("put again");
        assert_eq!(cache.failed_at("library/redis").await, 0);
    }

    #[tokio::test]
    async fn mark_failure_without_a_row_creates_an_empty_cache_entry() {
        let (cache, _dir) = cache().await;
        cache.mark_failure("library/solr", 150).await.expect("mark");
        assert_eq!(
            cache.get("library/solr").await.expect("get"),
            Some(CachedTags {
                tags: Vec::new(),
                fetched_at: 0,
                failed_at: 150,
            })
        );
    }
}
