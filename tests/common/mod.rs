use std::collections::HashMap;

use dokku_ui::settings::Settings;
use dokku_ui::storage;
use dokku_ui::web::AppState;
use tempfile::TempDir;

pub async fn test_state() -> (AppState, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let database_url = format!("sqlite://{}/test.db", dir.path().display());
    let pool = storage::connect(&database_url).await.expect("connect db");
    let settings = Settings::from_map(&HashMap::new()).expect("default settings");
    (AppState { db: pool, settings }, dir)
}
