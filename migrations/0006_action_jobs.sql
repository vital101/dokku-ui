CREATE TABLE action_jobs (
  id              TEXT PRIMARY KEY,
  run_id          TEXT NOT NULL REFERENCES action_runs(id) ON DELETE CASCADE,
  payload         TEXT NOT NULL,
  state           TEXT NOT NULL,
  attempts        INTEGER NOT NULL DEFAULT 0,
  max_attempts    INTEGER NOT NULL DEFAULT 3,
  available_at    INTEGER NOT NULL,
  lease_expires_at INTEGER,
  claimed_by      TEXT,
  last_error      TEXT,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);

CREATE INDEX idx_action_jobs_claim ON action_jobs(state, available_at);
CREATE INDEX idx_action_jobs_run ON action_jobs(run_id);