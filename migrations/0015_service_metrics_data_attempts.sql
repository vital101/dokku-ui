-- Attempts for data-dir size samples. Recording failures keeps the metrics
-- poller from retrying `du` on every cheap stats interval when a scan times
-- out or the service data directory is unavailable.
CREATE TABLE metrics_data_attempts (
  plugin          TEXT    NOT NULL,
  service         TEXT    NOT NULL,
  last_attempt_ms INTEGER NOT NULL,
  PRIMARY KEY (plugin, service)
);

-- Previous root-directory samples for memcached/nats measured the container
-- writable layer, not service data.
DELETE FROM service_data_sizes WHERE plugin IN ('memcached', 'nats');
