use std::sync::Arc;

use sqlx::SqlitePool;

use crate::dokku::{CapabilitiesStore, DokkuClient, SnapshotStore};
use crate::settings::Settings;
use crate::storage::jobs::SqliteJobsRepo;
use crate::storage::runs::SqliteRunsRepo;
use crate::storage::webhooks::SqliteWebhooksRepo;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub settings: Settings,
    pub dokku: Arc<dyn DokkuClient>,
    pub snapshot: Arc<SnapshotStore>,
    pub capabilities: Arc<CapabilitiesStore>,
    pub action_runs: Arc<SqliteRunsRepo>,
    pub jobs: Arc<SqliteJobsRepo>,
    pub webhooks: Arc<SqliteWebhooksRepo>,
}
