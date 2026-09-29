use sqlx::SqlitePool;

use crate::settings::Settings;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub settings: Settings,
}
