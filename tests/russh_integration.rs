use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dokku_ui::dokku::{DokkuClient, RusshClient};
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::domain::parse::{parse_apps_list, parse_ps_report};
use dokku_ui::settings::Settings;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use tempfile::TempDir;

const CLIENT_KEY: &str = r#"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACCzbwX3NbbLK7Cmu4Rl1J5aFqA6IirMVPGJzX9fode/AwAAAJh08OIgdPDi
IAAAAAtzc2gtZWQyNTUxOQAAACCzbwX3NbbLK7Cmu4Rl1J5aFqA6IirMVPGJzX9fode/Aw
AAAEB/V2EkWZzqbAgJMMyfyhphcUw5vB+Jnxzw+rydkdJ/TLNvBfc1tssrsKa7hGXUnloW
oDoiKsxU8YnNf1+h178DAAAAFWphY2tATWFjLmh5cGVyaW9uLmxhbg==
-----END OPENSSH PRIVATE KEY-----"#;

const HOST_KEY: &str = r#"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACCdgaKCWS6H4SjXphW9axTQGC5Zm9H+1y72KPGCwI+DQwAAAJiecnf8nnJ3
/AAAAAtzc2gtZWQyNTUxOQAAACCdgaKCWS6H4SjXphW9axTQGC5Zm9H+1y72KPGCwI+DQw
AAAEB7P5LeMSaA4no7ja1k70GczXC0xwVyIJVS8rCzu3W9vJ2BooJZLofhKNemFb1rFNAY
Llmb0f7XLvYo8YLAj4NDAAAAFWphY2tATWFjLmh5cGVyaW9uLmxhbg==
-----END OPENSSH PRIVATE KEY-----"#;

struct FakeDokku;

impl server::Handler for FakeDokku {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _user: &str, _key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        match command.as_str() {
            "apps:list" => {
                session.data(channel, "=====> My Apps\nmyapp\napi.internal\n")?;
                session.exit_status_request(channel, 0)?;
            }
            "ps:report myapp --format json" => {
                session.data(
                    channel,
                    r#"{"deployed":"true","running":"true","processes":"1","status-web":"running (CID: 1234567890abc)"}"#,
                )?;
                session.exit_status_request(channel, 0)?;
            }
            _ => {
                session.extended_data(channel, 1, "unknown command\n")?;
                session.exit_status_request(channel, 1)?;
            }
        }
        session.close(channel)?;
        Ok(())
    }
}

async fn client(addr: std::net::SocketAddr, key_path: &std::path::Path) -> RusshClient {
    let mut vars = HashMap::new();
    vars.insert("DOKKU_HOST".to_owned(), addr.ip().to_string());
    vars.insert("DOKKU_SSH_PORT".to_owned(), addr.port().to_string());
    vars.insert("DOKKU_SSH_USER".to_owned(), "dokku".to_owned());
    vars.insert(
        "DOKKU_SSH_KEY_PATH".to_owned(),
        key_path.display().to_string(),
    );
    let settings = Settings::from_map(&vars).expect("settings");
    RusshClient::new(&settings)
}

#[tokio::test]
async fn executes_command_and_reads_stdout() {
    let (addr, server, _connections) = spawn_fake_dokku().await;

    let dir = TempDir::new().expect("temp dir");
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(&key_path, CLIENT_KEY).expect("write key");

    let client = client(addr, &key_path).await;
    let output = client
        .exec(&DokkuCommand::AppsList)
        .await
        .expect("apps:list succeeds");
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, "=====> My Apps\nmyapp\napi.internal\n");
    assert_eq!(
        parse_apps_list(&output.stdout),
        vec!["myapp".to_owned(), "api.internal".to_owned()]
    );

    server.abort();
}

#[tokio::test]
async fn streams_stderr_and_exit_code_for_failures() {
    let (addr, server, _connections) = spawn_fake_dokku().await;

    let dir = TempDir::new().expect("temp dir");
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(&key_path, CLIENT_KEY).expect("write key");

    let client = client(addr, &key_path).await;
    let err = client
        .exec(&DokkuCommand::PsStart {
            app: dokku_ui::domain::AppName::try_from("myapp").expect("app name"),
        })
        .await
        .expect_err("unknown command fails");
    assert!(matches!(
        err,
        dokku_ui::dokku::DokkuError::Exit { code: 1, .. }
    ));

    server.abort();
}

#[tokio::test]
async fn output_parses_with_domain_parsers() {
    let (addr, server, _connections) = spawn_fake_dokku().await;

    let dir = TempDir::new().expect("temp dir");
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(&key_path, CLIENT_KEY).expect("write key");

    let client = client(addr, &key_path).await;
    let output = client
        .exec(&DokkuCommand::PsReport {
            app: dokku_ui::domain::AppName::try_from("myapp").expect("app name"),
        })
        .await
        .expect("ps:report succeeds");
    let report = parse_ps_report(&output.stdout).expect("parses");
    assert!(report.deployed);
    assert!(report.running);
    assert_eq!(report.process_count, 1);

    server.abort();
}

#[tokio::test]
async fn reuses_one_connection_across_many_commands() {
    let (addr, server, connections) = spawn_fake_dokku().await;

    let dir = TempDir::new().expect("temp dir");
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(&key_path, CLIENT_KEY).expect("write key");

    let client = client(addr, &key_path).await;
    let app = dokku_ui::domain::AppName::try_from("myapp").expect("app name");

    let outputs = [
        client.exec(&DokkuCommand::AppsList).await,
        client
            .exec(&DokkuCommand::PsReport { app: app.clone() })
            .await,
        client.exec(&DokkuCommand::AppsList).await,
        client.exec(&DokkuCommand::PsReport { app }).await,
    ];
    for output in &outputs {
        assert!(
            output.is_ok(),
            "every command succeeds on the reused session"
        );
    }
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "persistent session: one TCP connection for all commands"
    );

    server.abort();
}

#[tokio::test]
async fn failing_commands_do_not_tear_down_the_session() {
    let (addr, server, connections) = spawn_fake_dokku().await;

    let dir = TempDir::new().expect("temp dir");
    let key_path = dir.path().join("id_ed25519");
    std::fs::write(&key_path, CLIENT_KEY).expect("write key");

    let client = client(addr, &key_path).await;
    let app = dokku_ui::domain::AppName::try_from("myapp").expect("app name");

    let err = client.exec(&DokkuCommand::PsStart { app }).await;
    assert!(
        matches!(err, Err(dokku_ui::dokku::DokkuError::Exit { code: 1, .. })),
        "unknown command exits nonzero"
    );

    let output = client.exec(&DokkuCommand::AppsList).await;
    assert!(output.is_ok(), "session survives a failed command");
    assert_eq!(connections.load(Ordering::SeqCst), 1);

    server.abort();
}

async fn spawn_fake_dokku() -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let connections = Arc::new(AtomicUsize::new(0));

    let host_key = PrivateKey::from_openssh(HOST_KEY).expect("parse host key");
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        ..server::Config::default()
    });

    let connections_for_task = connections.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            connections_for_task.fetch_add(1, Ordering::SeqCst);
            let config = config.clone();
            tokio::spawn(async move {
                let _ = server::run_stream(config, stream, FakeDokku).await;
            });
        }
    });

    (addr, server, connections)
}
