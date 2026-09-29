use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russh::client;
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::{PublicKeyOrCertificate, decode_secret_key, key::PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect};

use crate::domain::command::DokkuCommand;
use crate::settings::Settings;

use super::client::{DokkuClient, DokkuError, DokkuOutput};

pub struct RusshClient {
    config: Arc<client::Config>,
    host: String,
    port: u16,
    user: String,
    key_path: PathBuf,
    known_hosts_path: Option<PathBuf>,
    timeout: Duration,
}

impl RusshClient {
    pub fn new(settings: &Settings) -> Self {
        let config = client::Config {
            nodelay: true,
            keepalive_interval: Some(Duration::from_secs(30)),
            ..client::Config::default()
        };
        Self {
            config: Arc::new(config),
            host: settings.dokku_host.clone(),
            port: settings.dokku_ssh_port,
            user: settings.dokku_ssh_user.clone(),
            key_path: settings.dokku_ssh_key_path.clone(),
            known_hosts_path: settings.dokku_ssh_known_hosts_path.clone(),
            timeout: Duration::from_secs(settings.command_timeout_secs),
        }
    }

    async fn run_command(&self, command: &str) -> Result<DokkuOutput, DokkuError> {
        let handler = HostKeyHandler {
            host: self.host.clone(),
            port: self.port,
            known_hosts_path: self.known_hosts_path.clone(),
        };
        let mut session = client::connect(
            self.config.clone(),
            (self.host.as_str(), self.port),
            handler,
        )
        .await
        .map_err(|err| DokkuError::Connect(err.to_string()))?;

        let key = Self::load_key(&self.key_path)?;
        let auth = session
            .authenticate_publickey(self.user.clone(), key)
            .await
            .map_err(|err| DokkuError::Connect(err.to_string()))?;
        if !auth.success() {
            return Err(DokkuError::AuthFailed);
        }

        let mut channel = session
            .channel_open_session()
            .await
            .map_err(|err| DokkuError::Connect(err.to_string()))?;
        channel
            .exec(true, command)
            .await
            .map_err(|err| DokkuError::Connect(err.to_string()))?;

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = -1;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    stdout.push_str(&String::from_utf8_lossy(&data));
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    stderr.push_str(&String::from_utf8_lossy(&data));
                }
                ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status as i32,
                _ => {}
            }
        }

        let _ = session.disconnect(Disconnect::ByApplication, "", "").await;

        match exit_code {
            0 => Ok(DokkuOutput {
                exit_code: 0,
                stdout,
                stderr,
            }),
            code => Err(DokkuError::Exit { code, stderr }),
        }
    }

    fn load_key(path: &Path) -> Result<PrivateKeyWithHashAlg, DokkuError> {
        let secret = std::fs::read_to_string(path).map_err(|err| DokkuError::KeyLoad {
            path: path.to_path_buf(),
            error: err.to_string(),
        })?;
        let key = decode_secret_key(&secret, None)
            .map_err(|err| DokkuError::KeyParse(err.to_string()))?;
        Ok(PrivateKeyWithHashAlg::new(Arc::new(key), None))
    }
}

#[async_trait]
impl DokkuClient for RusshClient {
    async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
        let remote_command = command.argv().join(" ");
        tokio::time::timeout(self.timeout, self.run_command(&remote_command))
            .await
            .map_err(|_| DokkuError::Timeout {
                secs: self.timeout.as_secs(),
            })?
    }
}

struct HostKeyHandler {
    host: String,
    port: u16,
    known_hosts_path: Option<PathBuf>,
}

impl client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let Some(path) = &self.known_hosts_path else {
            return Ok(true);
        };
        let key = server_public_key.public_key();
        match check_known_hosts_path(&self.host, self.port, &key, path) {
            Ok(true) => Ok(true),
            Ok(false) => match learn_known_hosts_path(&self.host, self.port, &key, path) {
                Ok(()) => Ok(true),
                Err(err) => {
                    tracing::warn!(error = %err, "failed to record host key");
                    Ok(false)
                }
            },
            Err(err) => {
                tracing::warn!(error = %err, "host key verification failed");
                Ok(false)
            }
        }
    }
}
