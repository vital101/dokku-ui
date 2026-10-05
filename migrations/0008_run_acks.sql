CREATE TABLE action_run_acks (
  run_id          TEXT NOT NULL REFERENCES action_runs(id) ON DELETE CASCADE,
  user_id         INTEGER NOT NULL,
  acknowledged_at INTEGER NOT NULL,
  PRIMARY KEY (run_id, user_id)
);