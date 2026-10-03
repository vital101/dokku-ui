use dokku_ui::dokku::{DokkuClient, RusshClient};
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
