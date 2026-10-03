use std::time::Instant;

use dokku_ui::dokku::{DokkuClient, RusshClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::parse::parse_apps_list;
use dokku_ui::settings::Settings;

#[tokio::test]
#[ignore = "requires DOKKU_HOST, DOKKU_SSH_KEY_PATH, DOKKU_SSH_USER pointing at a live dokku host"]
async fn live_host_lists_apps() {
    let settings = Settings::from_env().expect("settings from env");
    let client = RusshClient::new(&settings);

    let output = client
        .exec(&DokkuCommand::AppsList)
        .await
        .expect("apps:list succeeds against live host");
    let apps = parse_apps_list(&output.stdout);
    assert!(!apps.is_empty(), "live host should list at least one app");
    tracing::info!(apps = ?apps, "listed apps from live host");
}

#[tokio::test]
#[ignore = "requires DOKKU_HOST, DOKKU_SSH_KEY_PATH, DOKKU_SSH_USER pointing at a live dokku host"]
async fn live_host_survives_dashboard_sized_burst() {
    let settings = Settings::from_env().expect("settings from env");
    let client = RusshClient::new(&settings);

    let started = Instant::now();
    let output = client
        .exec(&DokkuCommand::AppsList)
        .await
        .expect("apps:list succeeds against live host");
    let apps = parse_apps_list(&output.stdout);

    for name in apps.iter().take(10) {
        let app = AppName::try_from(name.to_owned()).expect("valid app name");
        client
            .exec(&DokkuCommand::PsReport { app })
            .await
            .unwrap_or_else(|err| panic!("ps:report for {name} failed: {err}"));
    }

    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 15,
        "dashboard-sized burst took {elapsed:?}; a fresh connection per command would trip the host rate limit"
    );
    tracing::info!(elapsed = ?elapsed, apps = apps.len(), "burst completed");
}
