use std::collections::HashMap;
use std::sync::Arc;

use dokku_ui::dokku::MockClient;
use dokku_ui::settings::Settings;
use dokku_ui::storage;
use dokku_ui::web::AppState;
use tempfile::TempDir;

pub async fn test_state() -> (AppState, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let database_url = format!("sqlite://{}/test.db", dir.path().display());
    let pool = storage::connect(&database_url).await.expect("connect db");
    let settings = Settings::from_map(&HashMap::new()).expect("default settings");
    let dokku: Arc<dyn dokku_ui::dokku::DokkuClient> = Arc::new(MockClient::new());
    (
        AppState {
            db: pool,
            settings,
            dokku,
        },
        dir,
    )
}
