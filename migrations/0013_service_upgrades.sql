-- Cached Docker Hub tag lists (shared across containers) and the last
-- custom-env the UI set for a service (the host's plugin generation cannot
-- report it back, and `<plugin>:upgrade` clears env when resubmitted empty).
CREATE TABLE image_tag_cache (
  repo       TEXT PRIMARY KEY,
  tags_json  TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  failed_at  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE service_image_config (
  plugin     TEXT NOT NULL,
  service    TEXT NOT NULL,
  custom_env TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (plugin, service)
);
