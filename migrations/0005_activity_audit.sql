ALTER TABLE action_runs ADD COLUMN actor_user_id INTEGER;
ALTER TABLE action_runs ADD COLUMN actor_email TEXT;
ALTER TABLE action_runs ADD COLUMN operation TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE action_runs ADD COLUMN target_kind TEXT NOT NULL DEFAULT 'system';
ALTER TABLE action_runs ADD COLUMN parent_run_id TEXT REFERENCES action_runs(id) ON DELETE SET NULL;
ALTER TABLE action_runs ADD COLUMN attempts INTEGER NOT NULL DEFAULT 1;

CREATE INDEX idx_action_runs_target ON action_runs(target_kind, subject, created_at);
CREATE INDEX idx_action_runs_actor ON action_runs(actor_user_id, created_at);
CREATE INDEX idx_action_runs_parent ON action_runs(parent_run_id);