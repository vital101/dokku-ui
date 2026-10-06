use std::time::Instant;

use dokku_ui::dokku::{DokkuClient, RusshClient};
use dokku_ui::domain::AppName;
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::parse::{
    parse_apps_list, parse_certs_report, parse_git_public_key, parse_git_report,
    parse_letsencrypt_list, parse_logs_failed,
};
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

/// Read-only P2 smoke: the Deploy/TLS/logs-failed reads the UI depends on.
/// Nothing here mutates the host (no git:sync, no letsencrypt enable).
#[tokio::test]
#[ignore = "requires DOKKU_HOST, DOKKU_SSH_KEY_PATH, DOKKU_SSH_USER pointing at a live dokku host"]
async fn live_host_serves_deploy_and_tls_reads() {
    let settings = Settings::from_env().expect("settings from env");
    let client = RusshClient::new(&settings);

    let output = client
        .exec(&DokkuCommand::AppsList)
        .await
        .expect("apps:list succeeds against live host");
    let apps = parse_apps_list(&output.stdout);
    let app = AppName::try_from(apps[0].clone()).expect("valid app name");

    let output = client
        .exec(&DokkuCommand::GitReport { app: app.clone() })
        .await
        .expect("git:report succeeds");
    let report = parse_git_report(&output.stdout);
    assert!(
        !report.computed_deploy_branch.is_empty(),
        "computed deploy branch should parse: {report:?}"
    );

    // The host has no generated deploy key: exit 1 with the guidance block.
    match client.exec(&DokkuCommand::GitPublicKey).await {
        Ok(output) => {
            if let Some(key) = parse_git_public_key(&output.stdout) {
                assert!(key.starts_with("ssh-"), "unexpected key line: {key}");
            }
        }
        Err(dokku_ui::dokku::DokkuError::Exit { .. }) => {}
        Err(err) => panic!("git:public-key failed unexpectedly: {err}"),
    }

    if let Ok(output) = client.exec(&DokkuCommand::LetsencryptList).await {
        let entries = parse_letsencrypt_list(&output.stdout);
        tracing::info!(entries = entries.len(), "letsencrypt:list parsed");
    }

    let output = client
        .exec(&DokkuCommand::CertsReport {
            app: Some(app.clone()),
        })
        .await
        .expect("certs:report succeeds");
    assert!(
        parse_certs_report(&output.stdout).is_some(),
        "certs:report should yield an ssl section"
    );

    let output = client
        .exec(&DokkuCommand::LogsFailed { app })
        .await
        .expect("logs:failed succeeds");
    let _ = parse_logs_failed(&output.stdout);
}
