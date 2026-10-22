use sqlx::SqlitePool;

use crate::domain::metrics::{DataSizeAggregate, DataSizeSample, MetricAggregate, MetricSample};
use crate::domain::{ServiceName, ServicePlugin};

/// Durable metric samples, written by the metrics poller and read by every
/// container from the shared SQLite file. The unique bucket indexes are the
/// cross-process dedupe mechanism: several containers sampling the same
/// service in the same bucket collapse into one row.
#[derive(Debug, Clone)]
pub struct SqliteMetrics {
    pool: SqlitePool,
}

impl SqliteMetrics {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record_sample(&self, sample: &MetricSample) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO service_metric_samples ( \
                 plugin, service, ts, bucket, cpu_percent, cpu_total_usec, cpus, \
                 mem_used_bytes, mem_limit_bytes, host_mem_total_kb, fs_total_kb, \
                 fs_used_kb, fs_avail_kb \
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(plugin, service, bucket) DO NOTHING",
        )
        .bind(&sample.plugin)
        .bind(&sample.service)
        .bind(sample.ts_ms)
        .bind(sample.bucket_ms)
        .bind(sample.cpu_percent)
        .bind(sample.cpu_total_usec)
        .bind(sample.cpus)
        .bind(sample.mem_used_bytes)
        .bind(sample.mem_limit_bytes)
        .bind(sample.host_mem_total_kb)
        .bind(sample.fs_total_kb)
        .bind(sample.fs_used_kb)
        .bind(sample.fs_avail_kb)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_data_size(&self, sample: &DataSizeSample) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO service_data_sizes (plugin, service, ts, bucket, data_kb) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(plugin, service, bucket) DO NOTHING",
        )
        .bind(&sample.plugin)
        .bind(&sample.service)
        .bind(sample.ts_ms)
        .bind(sample.bucket_ms)
        .bind(sample.data_kb)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn latest_data_kb(
        &self,
        plugin: ServicePlugin,
        service: &ServiceName,
    ) -> Result<Option<u64>, sqlx::Error> {
        let value: Option<i64> = sqlx::query_scalar(
            "SELECT data_kb FROM service_data_sizes \
             WHERE plugin = ? AND service = ? \
             ORDER BY ts DESC \
             LIMIT 1",
        )
        .bind(plugin.as_str())
        .bind(service.as_str())
        .fetch_optional(&self.pool)
        .await?;
        Ok(value.and_then(|value| u64::try_from(value).ok()))
    }

    pub async fn claim_data_attempt(
        &self,
        plugin: ServicePlugin,
        service: &ServiceName,
        now_ms: i64,
        interval_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        if interval_ms <= 0 {
            return Ok(false);
        }
        let threshold = now_ms.saturating_sub(interval_ms);
        let result = sqlx::query(
            "INSERT INTO metrics_data_attempts (plugin, service, last_attempt_ms) \
             VALUES (?, ?, ?) \
             ON CONFLICT(plugin, service) DO UPDATE SET last_attempt_ms = excluded.last_attempt_ms \
             WHERE metrics_data_attempts.last_attempt_ms <= ?",
        )
        .bind(plugin.as_str())
        .bind(service.as_str())
        .bind(now_ms)
        .bind(threshold)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn aggregates(
        &self,
        plugin: ServicePlugin,
        service: &ServiceName,
        start_ms: i64,
        width_ms: i64,
    ) -> Result<Vec<MetricAggregate>, sqlx::Error> {
        let width_ms = width_ms.max(1);
        let rows = sqlx::query_as::<_, (i64, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>)>(
            "SELECT \
                  ((ts / ?) * ?) AS bucket_ts, \
                  AVG(cpu_percent) AS cpu_percent, \
                  AVG(CASE \
                      WHEN mem_used_bytes IS NOT NULL \
                          AND COALESCE(NULLIF(mem_limit_bytes, 0), NULLIF(host_mem_total_kb * 1024, 0)) IS NOT NULL \
                      THEN mem_used_bytes * 100.0 / \
                          COALESCE(NULLIF(mem_limit_bytes, 0), NULLIF(host_mem_total_kb * 1024, 0)) \
                      ELSE NULL \
                  END) AS memory_percent, \
                  AVG(CASE \
                      WHEN fs_used_kb IS NOT NULL AND fs_total_kb > 0 \
                      THEN fs_used_kb * 100.0 / fs_total_kb \
                      ELSE NULL \
                  END) AS disk_percent, \
                  AVG(fs_used_kb * 1024.0) AS fs_used_bytes, \
                  MAX(fs_total_kb * 1024.0) AS fs_total_bytes \
              FROM service_metric_samples \
              WHERE plugin = ? AND service = ? AND ts >= ? \
              GROUP BY bucket_ts \
              ORDER BY bucket_ts ASC",
        )
        .bind(width_ms)
        .bind(width_ms)
        .bind(plugin.as_str())
        .bind(service.as_str())
        .bind(start_ms)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    ts_ms,
                    cpu_percent,
                    memory_percent,
                    disk_percent,
                    fs_used_bytes,
                    fs_total_bytes,
                )| {
                    MetricAggregate {
                        ts_ms,
                        cpu_percent,
                        memory_percent,
                        disk_percent,
                        fs_used_bytes,
                        fs_total_bytes,
                    }
                },
            )
            .collect())
    }

    pub async fn data_aggregates(
        &self,
        plugin: ServicePlugin,
        service: &ServiceName,
        start_ms: i64,
        width_ms: i64,
    ) -> Result<Vec<DataSizeAggregate>, sqlx::Error> {
        let width_ms = width_ms.max(1);
        let rows = sqlx::query_as::<_, (i64, Option<f64>)>(
            "SELECT \
                  ((ts / ?) * ?) AS bucket_ts, \
                  AVG(data_kb * 1024.0) AS data_bytes \
              FROM service_data_sizes \
              WHERE plugin = ? AND service = ? AND ts >= ? \
              GROUP BY bucket_ts \
              ORDER BY bucket_ts ASC",
        )
        .bind(width_ms)
        .bind(width_ms)
        .bind(plugin.as_str())
        .bind(service.as_str())
        .bind(start_ms)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(ts_ms, data_bytes)| DataSizeAggregate { ts_ms, data_bytes })
            .collect())
    }

    pub async fn claim_poll(&self, now_ms: i64, interval_ms: i64) -> Result<bool, sqlx::Error> {
        if interval_ms <= 0 {
            return Ok(false);
        }

        sqlx::query(
            "INSERT INTO metrics_poll (id, next_due_ms) VALUES (1, 0) \
             ON CONFLICT(id) DO NOTHING",
        )
        .execute(&self.pool)
        .await?;
        let result = sqlx::query(
            "UPDATE metrics_poll SET next_due_ms = ? WHERE id = 1 AND next_due_ms <= ?",
        )
        .bind(now_ms.saturating_add(interval_ms))
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn prune(&self, older_than_ms: i64) -> Result<u64, sqlx::Error> {
        let samples = sqlx::query("DELETE FROM service_metric_samples WHERE ts < ?")
            .bind(older_than_ms)
            .execute(&self.pool)
            .await?
            .rows_affected();
        let data_sizes = sqlx::query("DELETE FROM service_data_sizes WHERE ts < ?")
            .bind(older_than_ms)
            .execute(&self.pool)
            .await?
            .rows_affected();
        sqlx::query("DELETE FROM metrics_data_attempts WHERE last_attempt_ms < ?")
            .bind(older_than_ms)
            .execute(&self.pool)
            .await?;
        Ok(samples + data_sizes)
    }
}

#[cfg(test)]
mod tests {
    use crate::storage;

    use super::*;

    async fn store() -> (SqliteMetrics, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/metrics.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteMetrics::new(pool), dir)
    }

    fn redis() -> ServicePlugin {
        ServicePlugin::try_from("redis").expect("redis plugin")
    }

    fn candid() -> ServiceName {
        ServiceName::try_from("candid").expect("service name")
    }

    fn sample(plugin: ServicePlugin, service: &ServiceName, ts_ms: i64) -> MetricSample {
        MetricSample {
            plugin: plugin.to_string(),
            service: service.to_string(),
            ts_ms,
            bucket_ms: ts_ms - ts_ms.rem_euclid(60_000),
            cpu_percent: Some(10.0),
            cpu_total_usec: Some(1_000_000),
            cpus: Some(2),
            mem_used_bytes: Some(2048),
            mem_limit_bytes: Some(4096),
            host_mem_total_kb: Some(1_048_576),
            fs_total_kb: Some(100),
            fs_used_kb: Some(25),
            fs_avail_kb: Some(75),
        }
    }

    fn data_size(plugin: ServicePlugin, service: &ServiceName, ts_ms: i64) -> DataSizeSample {
        DataSizeSample {
            plugin: plugin.to_string(),
            service: service.to_string(),
            ts_ms,
            bucket_ms: ts_ms - ts_ms.rem_euclid(900_000),
            data_kb: 12,
        }
    }

    async fn count_samples(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM service_metric_samples")
            .fetch_one(pool)
            .await
            .expect("count samples")
    }

    async fn count_data_sizes(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM service_data_sizes")
            .fetch_one(pool)
            .await
            .expect("count data sizes")
    }

    #[tokio::test]
    async fn record_sample_dedupes_by_bucket() {
        let (metrics, _dir) = store().await;
        metrics
            .record_sample(&sample(redis(), &candid(), 60_000))
            .await
            .expect("record first");
        metrics
            .record_sample(&MetricSample {
                cpu_percent: Some(90.0),
                ..sample(redis(), &candid(), 90_000)
            })
            .await
            .expect("record duplicate bucket");

        assert_eq!(count_samples(&metrics.pool).await, 1);
    }

    #[tokio::test]
    async fn record_data_size_dedupes_by_bucket() {
        let (metrics, _dir) = store().await;
        metrics
            .record_data_size(&data_size(redis(), &candid(), 900_000))
            .await
            .expect("record first");
        metrics
            .record_data_size(&data_size(redis(), &candid(), 910_000))
            .await
            .expect("record duplicate bucket");

        assert_eq!(count_data_sizes(&metrics.pool).await, 1);
    }

    #[tokio::test]
    async fn aggregates_average_raw_samples_by_requested_width() {
        let (metrics, _dir) = store().await;
        metrics
            .record_sample(&sample(redis(), &candid(), 60_000))
            .await
            .expect("first");
        metrics
            .record_sample(&MetricSample {
                cpu_percent: Some(30.0),
                mem_used_bytes: Some(4096),
                fs_used_kb: Some(50),
                ..sample(redis(), &candid(), 120_000)
            })
            .await
            .expect("second");

        let rows = metrics
            .aggregates(redis(), &candid(), 0, 600_000)
            .await
            .expect("aggregates");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ts_ms, 0);
        assert_eq!(rows[0].cpu_percent, Some(20.0));
        assert_eq!(rows[0].memory_percent, Some(75.0));
        assert_eq!(rows[0].disk_percent, Some(37.5));
        assert_eq!(rows[0].fs_used_bytes, Some(38_400.0));
        assert_eq!(rows[0].fs_total_bytes, Some(102_400.0));
    }

    #[tokio::test]
    async fn data_aggregates_average_bytes() {
        let (metrics, _dir) = store().await;
        metrics
            .record_data_size(&data_size(redis(), &candid(), 0))
            .await
            .expect("first");
        metrics
            .record_data_size(&DataSizeSample {
                data_kb: 24,
                ..data_size(redis(), &candid(), 900_000)
            })
            .await
            .expect("second");

        let rows = metrics
            .data_aggregates(redis(), &candid(), 0, 1_800_000)
            .await
            .expect("aggregates");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].data_bytes, Some(18_432.0));
    }

    #[tokio::test]
    async fn claim_poll_lets_one_poller_per_interval() {
        let (metrics, _dir) = store().await;

        assert!(metrics.claim_poll(0, 60_000).await.expect("first claim"));
        assert!(
            !metrics
                .claim_poll(1_000, 60_000)
                .await
                .expect("second claim")
        );
        assert!(metrics.claim_poll(60_000, 60_000).await.expect("due claim"));
        assert!(!metrics.claim_poll(0, 0).await.expect("disabled claim"));
    }

    #[tokio::test]
    async fn prune_removes_samples_and_data_sizes() {
        let (metrics, _dir) = store().await;
        metrics
            .record_sample(&sample(redis(), &candid(), 1_000))
            .await
            .expect("old sample");
        metrics
            .record_sample(&sample(redis(), &candid(), 70_000))
            .await
            .expect("new sample");
        metrics
            .record_data_size(&data_size(redis(), &candid(), 0))
            .await
            .expect("old data");

        assert_eq!(metrics.prune(5_000).await.expect("prune"), 2);
        assert_eq!(count_samples(&metrics.pool).await, 1);
        assert_eq!(count_data_sizes(&metrics.pool).await, 0);
    }

    #[tokio::test]
    async fn latest_data_kb_reads_the_newest_row() {
        let (metrics, _dir) = store().await;
        assert_eq!(
            metrics
                .latest_data_kb(redis(), &candid())
                .await
                .expect("latest"),
            None
        );

        metrics
            .record_data_size(&DataSizeSample {
                data_kb: 18,
                ..data_size(redis(), &candid(), 1_000)
            })
            .await
            .expect("record");
        metrics
            .record_data_size(&data_size(redis(), &candid(), 900_000))
            .await
            .expect("record");

        assert_eq!(
            metrics
                .latest_data_kb(redis(), &candid())
                .await
                .expect("latest"),
            Some(12)
        );
    }

    #[tokio::test]
    async fn claim_data_attempt_spaces_failed_attempts() {
        let (metrics, _dir) = store().await;

        assert!(
            metrics
                .claim_data_attempt(redis(), &candid(), 1_000, 900_000)
                .await
                .expect("first attempt")
        );
        assert!(
            !metrics
                .claim_data_attempt(redis(), &candid(), 10_000, 900_000)
                .await
                .expect("second attempt")
        );
        assert!(
            metrics
                .claim_data_attempt(redis(), &candid(), 901_000, 900_000)
                .await
                .expect("due attempt")
        );
        assert!(
            !metrics
                .claim_data_attempt(redis(), &candid(), 901_000, 0)
                .await
                .expect("disabled attempt")
        );
    }
}
