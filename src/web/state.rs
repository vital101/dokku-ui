use std::sync::Arc;

use sqlx::SqlitePool;

use crate::dokku::{ActionRuns, DokkuClient, SnapshotStore};
use crate::settings::Settings;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub settings: Settings,
    pub dokku: Arc<dyn DokkuClient>,
    pub snapshot: Arc<SnapshotStore>,
    pub action_runs: Arc<ActionRuns>,
}
