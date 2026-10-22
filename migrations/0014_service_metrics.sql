-- Time-series samples for the service Metrics tab. Written by the background
-- metrics poller and read by every container from the shared file.
--
-- `bucket` is `ts` aligned to the poll interval, and the unique index is what
-- makes multi-container polling safe: two containers sampling the same service
-- in the same bucket collapse into one row.
CREATE TABLE service_metric_samples (
  plugin            TEXT    NOT NULL,
  service           TEXT    NOT NULL,
  ts                INTEGER NOT NULL,
  bucket            INTEGER NOT NULL,
  cpu_percent       REAL,
  cpu_total_usec    INTEGER,
  cpus              INTEGER,
  mem_used_bytes    INTEGER,
  mem_limit_bytes   INTEGER,
  host_mem_total_kb INTEGER,
  fs_total_kb       INTEGER,
  fs_used_kb        INTEGER,
  fs_avail_kb       INTEGER
);

CREATE UNIQUE INDEX ux_metric_samples_bucket
  ON service_metric_samples (plugin, service, bucket);
CREATE INDEX ix_metric_samples_series
  ON service_metric_samples (plugin, service, ts DESC);

-- Data-dir size (`du`) is kept separate because it is sampled on a much slower
-- cadence than the cgroup/df script.
CREATE TABLE service_data_sizes (
  plugin  TEXT    NOT NULL,
  service TEXT    NOT NULL,
  ts      INTEGER NOT NULL,
  bucket  INTEGER NOT NULL,
  data_kb INTEGER NOT NULL
);

CREATE UNIQUE INDEX ux_data_sizes_bucket
  ON service_data_sizes (plugin, service, bucket);
CREATE INDEX ix_data_sizes_series
  ON service_data_sizes (plugin, service, ts DESC);

-- Singleton poll lease: exactly one container runs a poll round per interval,
-- so N containers do not multiply the SSH load on the dokku host.
CREATE TABLE metrics_poll (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  next_due_ms INTEGER NOT NULL DEFAULT 0
);

INSERT INTO metrics_poll (id, next_due_ms) VALUES (1, 0);
