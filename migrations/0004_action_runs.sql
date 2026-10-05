CREATE TABLE action_runs (
  id          TEXT PRIMARY KEY,
  subject     TEXT NOT NULL,
  outcome     TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  finished_at INTEGER
);

CREATE INDEX idx_action_runs_updated_at ON action_runs(updated_at);

CREATE TABLE action_run_lines (
  run_id TEXT NOT NULL REFERENCES action_runs(id) ON DELETE CASCADE,
  seq    INTEGER NOT NULL,
  line   TEXT NOT NULL,
  PRIMARY KEY (run_id, seq)
);
