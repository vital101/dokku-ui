//! Docker Hub image-tag lookup for the service upgrade selector.
//!
//! `ImageTagStore` serves a shared SQLite cache and refreshes it from a
//! swappable [`ImageTagSource`]. The real source is Docker Hub over HTTPS; the
//! trait lets tests inject a fake and keeps the network surface tiny.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::storage::image_tags::SqliteImageTagCache;

const DOCKER_HUB_BASE: &str = "https://hub.docker.com";
const PAGE_SIZE: usize = 100;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// After a failed refresh, wait this long before hitting Docker Hub again.
/// Stale tags (when present) keep serving during the window.
const FAILURE_BACKOFF_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageTagError {
    #[error("docker hub request failed: {0}")]
    Http(String),
    #[error("docker hub returned status {0}")]
    Status(u16),
    #[error("docker hub response was malformed: {0}")]
    Parse(String),
}

#[async_trait]
pub trait ImageTagSource: Send + Sync {
    async fn fetch(&self, repo: &str) -> Result<Vec<String>, ImageTagError>;
}

/// Fetches tag names from `https://hub.docker.com/v2/repositories/{repo}/tags`.
pub struct DockerHubSource {
    client: reqwest::Client,
}

impl DockerHubSource {
    pub fn new() -> Result<Self, ImageTagError> {
        let client = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(concat!("dokku-ui/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| ImageTagError::Http(err.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl ImageTagSource for DockerHubSource {
    async fn fetch(&self, repo: &str) -> Result<Vec<String>, ImageTagError> {
        let url = format!(
            "{DOCKER_HUB_BASE}/v2/repositories/{repo}/tags?page_size={PAGE_SIZE}&ordering=last_updated"
        );
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|err| ImageTagError::Http(err.to_string()))?;
        if !response.status().is_success() {
            return Err(ImageTagError::Status(response.status().as_u16()));
        }
        let body = response
            .text()
            .await
            .map_err(|err| ImageTagError::Http(err.to_string()))?;
        parse_tags(&body)
    }
}

/// Extracts tag names from a Docker Hub tags response body.
pub fn parse_tags(body: &str) -> Result<Vec<String>, ImageTagError> {
    #[derive(Deserialize)]
    struct TagsResponse {
        results: Vec<TagResult>,
    }
    #[derive(Deserialize)]
    struct TagResult {
        name: String,
    }
    let parsed: TagsResponse =
        serde_json::from_str(body).map_err(|err| ImageTagError::Parse(err.to_string()))?;
    Ok(parsed
        .results
        .into_iter()
        .map(|result| result.name)
        .collect())
}

/// Cache-backed tag lookup shared by every container.
pub struct ImageTagStore {
    source: Arc<dyn ImageTagSource>,
    cache: SqliteImageTagCache,
    ttl_secs: i64,
}

/// A source that never reaches the network (tests, and `ttl = 0`).
struct NoopImageTagSource;

#[async_trait]
impl ImageTagSource for NoopImageTagSource {
    async fn fetch(&self, _repo: &str) -> Result<Vec<String>, ImageTagError> {
        Ok(Vec::new())
    }
}

impl ImageTagStore {
    pub fn new(source: Arc<dyn ImageTagSource>, cache: SqliteImageTagCache, ttl_secs: u64) -> Self {
        Self {
            source,
            cache,
            ttl_secs: ttl_secs as i64,
        }
    }

    /// A store that never fetches tags (used when tag fetching is disabled).
    pub fn disabled(cache: SqliteImageTagCache) -> Self {
        Self::new(Arc::new(NoopImageTagSource), cache, 0)
    }

    /// `IMAGE_TAGS_TTL_SECS=0` disables remote fetching entirely.
    pub fn enabled(&self) -> bool {
        self.ttl_secs > 0
    }

    /// Returns the cached tags when fresh, otherwise refreshes from the
    /// source. A failed refresh serves the stale cache when present and
    /// records a short negative-cache window so repeated requests do not
    /// hammer Docker Hub. With no cached tags the first failure propagates;
    /// during the backoff the refresh is skipped and the caller falls back.
    pub async fn tags(&self, repo: &str) -> Result<Vec<String>, ImageTagError> {
        if !self.enabled() {
            return Ok(Vec::new());
        }
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let cached = self.cache.get(repo).await.ok().flatten();
        if let Some(cached) = &cached {
            if now.saturating_sub(cached.fetched_at) < self.ttl_secs {
                return Ok(cached.tags.clone());
            }
            if cached.failed_at > 0 && now.saturating_sub(cached.failed_at) < FAILURE_BACKOFF_SECS {
                return Ok(cached.tags.clone());
            }
        }
        match self.source.fetch(repo).await {
            Ok(tags) => {
                if let Err(err) = self.cache.put(repo, &tags, now).await {
                    tracing::warn!(error = %err, "failed to cache docker hub tags");
                }
                Ok(tags)
            }
            Err(err) => {
                if let Err(cache_err) = self.cache.mark_failure(repo, now).await {
                    tracing::warn!(error = %cache_err, "failed to record docker hub failure");
                }
                match cached {
                    Some(cached) if !cached.tags.is_empty() => {
                        tracing::warn!(repo, error = %err, "docker hub refresh failed; serving stale tags");
                        Ok(cached.tags)
                    }
                    _ => Err(err),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::TempDir;

    use super::*;
    use crate::storage;

    const TAGS_FIXTURE: &str = include_str!("../tests/fixtures/dockerhub_redis_tags.json");
    const POSTGRES_TAGS_FIXTURE: &str =
        include_str!("../tests/fixtures/dockerhub_postgres_tags.json");

    struct FakeSource {
        calls: AtomicUsize,
        result: Result<Vec<String>, ImageTagError>,
    }

    impl FakeSource {
        fn ok(tags: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                result: Ok(tags.iter().map(|tag| (*tag).to_owned()).collect()),
            })
        }

        fn err(err: ImageTagError) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                result: Err(err),
            })
        }
    }

    #[async_trait]
    impl ImageTagSource for FakeSource {
        async fn fetch(&self, _repo: &str) -> Result<Vec<String>, ImageTagError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    async fn pool() -> (sqlx::SqlitePool, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/tags.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (pool, dir)
    }

    #[test]
    fn parses_tag_names_from_a_docker_hub_response() {
        let tags = parse_tags(TAGS_FIXTURE).expect("parse");
        assert!(tags.contains(&"7.2.4".to_owned()), "{tags:?}");
        assert!(parse_tags("not json").is_err());
    }

    #[test]
    fn selects_postgres_two_component_tags() {
        let raw = parse_tags(POSTGRES_TAGS_FIXTURE).expect("parse");
        let tags = crate::domain::select_tags(&raw, Some("16.2"), 10);
        assert_eq!(tags[0], "16.2");
        assert!(tags.contains(&"18.6".to_owned()), "{tags:?}");
        assert!(tags.contains(&"17.11".to_owned()), "{tags:?}");
        assert!(!tags.contains(&"16".to_owned()), "{tags:?}");
        assert!(!tags.contains(&"latest".to_owned()), "{tags:?}");
    }

    #[tokio::test]
    async fn fetches_and_caches_on_a_miss() {
        let (pool, _dir) = pool().await;
        let source = FakeSource::ok(&["7.2.4"]);
        let store = ImageTagStore::new(source.clone(), SqliteImageTagCache::new(pool), 3600);

        assert_eq!(
            store.tags("library/redis").await.expect("tags"),
            vec!["7.2.4"]
        );
        // The second call is served from the fresh cache.
        assert_eq!(
            store.tags("library/redis").await.expect("tags"),
            vec!["7.2.4"]
        );
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn serves_stale_tags_when_refresh_fails() {
        let (pool, _dir) = pool().await;
        let cache = SqliteImageTagCache::new(pool);
        ImageTagStore::new(FakeSource::ok(&["7.2.4"]), cache.clone(), 3600)
            .tags("library/redis")
            .await
            .expect("prime");
        // Expire the row, then fail the source: the stale list still serves.
        cache
            .put("library/redis", &["7.2.4".to_owned()], 1)
            .await
            .expect("expire");
        let store = ImageTagStore::new(FakeSource::err(ImageTagError::Status(429)), cache, 3600);
        assert_eq!(
            store.tags("library/redis").await.expect("stale"),
            vec!["7.2.4"]
        );
    }

    #[tokio::test]
    async fn ttl_zero_disables_fetching() {
        let (pool, _dir) = pool().await;
        let source = FakeSource::ok(&["7.2.4"]);
        let store = ImageTagStore::new(source.clone(), SqliteImageTagCache::new(pool), 0);
        assert!(!store.enabled());
        assert!(store.tags("library/redis").await.expect("tags").is_empty());
        assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn errors_without_a_cache() {
        let (pool, _dir) = pool().await;
        let store = ImageTagStore::new(
            FakeSource::err(ImageTagError::Status(500)),
            SqliteImageTagCache::new(pool),
            3600,
        );
        assert!(store.tags("library/redis").await.is_err());
    }

    #[tokio::test]
    async fn backs_off_after_a_failure_without_a_cache() {
        let (pool, _dir) = pool().await;
        let source = FakeSource::err(ImageTagError::Status(503));
        let store = ImageTagStore::new(source.clone(), SqliteImageTagCache::new(pool), 3600);

        assert!(store.tags("library/solr").await.is_err());
        // Inside the negative-cache window the fetch is skipped and the
        // caller gets an empty list (which renders the text fallback).
        assert!(
            store
                .tags("library/solr")
                .await
                .expect("backoff")
                .is_empty()
        );
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_after_the_negative_cache_window() {
        let (pool, _dir) = pool().await;
        let cache = SqliteImageTagCache::new(pool);
        let expired = time::OffsetDateTime::now_utc().unix_timestamp() - FAILURE_BACKOFF_SECS - 1;
        cache
            .mark_failure("library/redis", expired)
            .await
            .expect("mark");
        let source = FakeSource::ok(&["7.2.4"]);
        let store = ImageTagStore::new(source.clone(), cache, 3600);
        assert_eq!(
            store.tags("library/redis").await.expect("retry"),
            vec!["7.2.4"]
        );
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }
}
