CREATE TABLE capabilities (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  data       TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);