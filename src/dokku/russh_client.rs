use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russh::client;
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::{PublicKeyOrCertificate, decode_secret_key, key::PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect};

use crate::domain::command::{CommandTimeout, DokkuCommand};
use crate::settings::Settings;

use super::client::{DokkuClient, DokkuError, DokkuOutput};

const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type Session = Arc<client::Handle<HostKeyHandler>>;

pub struct RusshClient {
    config: Arc<client::Config>,
    host: String,
    port: u16,
    user: String,
    key_path: PathBuf,
    known_hosts_path: Option<PathBuf>,
    timeout: Duration,
    session: tokio::sync::Mutex<Option<Session>>,
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
            session: tokio::sync::Mutex::new(None),
        }
    }

    async fn session_handle(&self) -> Result<Session, DokkuError> {
        let mut guard = self.session.lock().await;
        if let Some(session) = guard.as_ref() {
            if !session.is_closed() {
                return Ok(session.clone());
            }
            Self::close_session(guard.take().expect("checked above")).await;
        }
        let session = Arc::new(self.connect().await?);
        *guard = Some(session.clone());
        Ok(session)
    }

    async fn connect(&self) -> Result<client::Handle<HostKeyHandler>, DokkuError> {
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
        Ok(session)
    }

    async fn invalidate_if_current(&self, session: &Session) {
        let taken = {
            let mut guard = self.session.lock().await;
            match guard.as_ref() {
                Some(current) if Arc::ptr_eq(current, session) => guard.take(),
                _ => None,
            }
        };
        if let Some(stale) = taken {
            Self::close_session(stale).await;
        }
    }

    async fn run_command(
        &self,
        command: &str,
        sink: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> Result<DokkuOutput, DokkuError> {
        let session = self.session_handle().await?;
        let channel = match session.channel_open_session().await {
            Ok(channel) => channel,
            Err(_) => {
                self.invalidate_if_current(&session).await;
                let session = self.session_handle().await?;
                session
                    .channel_open_session()
                    .await
                    .map_err(|err| DokkuError::Connect(err.to_string()))?
            }
        };

        let output = match Self::exec_on_channel(channel, command, sink).await {
            Ok(output) => output,
            Err(err) => {
                self.invalidate_if_current(&session).await;
                return Err(err);
            }
        };
        finalize(output)
    }

    async fn exec_on_channel(
        mut channel: russh::Channel<russh::client::Msg>,
        command: &str,
        sink: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> Result<DokkuOutput, DokkuError> {
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
                    let text = String::from_utf8_lossy(&data).into_owned();
                    stdout.push_str(&text);
                    if let Some(sink) = &sink {
                        let _ = sink.send(text).await;
                    }
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    let text = String::from_utf8_lossy(&data).into_owned();
                    stderr.push_str(&text);
                    if let Some(sink) = &sink {
                        let _ = sink.send(text).await;
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status as i32,
                _ => {}
            }
        }

        Ok(DokkuOutput {
            exit_code,
            stdout,
            stderr,
        })
    }

    async fn close_session(session: Session) {
        let _ = tokio::time::timeout(
            DISCONNECT_TIMEOUT,
            session.disconnect(Disconnect::ByApplication, "", ""),
        )
        .await;
    }

    async fn invalidate_session(&self) {
        if let Some(session) = self.session.lock().await.take() {
            Self::close_session(session).await;
        }
    }

    fn command_timeout(&self, command: &DokkuCommand) -> Option<Duration> {
        match command.timeout() {
            CommandTimeout::Default => Some(self.timeout),
            CommandTimeout::Indefinite => None,
        }
    }

    /// Runs `command`, optionally streaming output chunks to `sink`. Action
    /// commands have no timeout; the rest are bounded by `COMMAND_TIMEOUT_SECS`.
    async fn execute(
        &self,
        command: &DokkuCommand,
        sink: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> Result<DokkuOutput, DokkuError> {
        let remote_command = shell_command(&command.argv());
        let timeout = self.command_timeout(command);
        let fut = self.run_command(&remote_command, sink);
        match timeout {
            Some(secs) => match tokio::time::timeout(secs, fut).await {
                Ok(result) => result,
                Err(_) => {
                    self.invalidate_session().await;
                    Err(DokkuError::Timeout {
                        secs: secs.as_secs(),
                    })
                }
            },
            None => fut.await,
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

fn finalize(output: DokkuOutput) -> Result<DokkuOutput, DokkuError> {
    match output.exit_code {
        0 => Ok(output),
        code => Err(DokkuError::Exit {
            code,
            stderr: output.stderr,
        }),
    }
}

/// Joins argv into the string the remote login shell executes. Arguments made
/// of shell-safe characters are passed through bare; anything else — including
/// the empty string `<plugin>:logs` uses as its follow-flag placeholder — is
/// single-quoted so it reaches dokku verbatim.
fn shell_command(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_arg(arg: &str) -> String {
    let bare = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@=+,".contains(c));
    if bare {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

#[async_trait]
impl DokkuClient for RusshClient {
    async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
        self.execute(command, None).await
    }

    async fn exec_streaming(
        &self,
        command: &DokkuCommand,
        sink: tokio::sync::mpsc::Sender<String>,
    ) -> Result<DokkuOutput, DokkuError> {
        self.execute(command, Some(sink)).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<russh::client::Handle<super::HostKeyHandler>>();
    }

    #[test]
    fn shell_command_leaves_plain_arguments_bare() {
        let argv = vec![
            "ps:report".to_owned(),
            "myapp".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
        ];
        assert_eq!(shell_command(&argv), "ps:report myapp --format json");
    }

    #[test]
    fn shell_command_quotes_the_empty_follow_flag_slot() {
        let argv = vec![
            "redis:logs".to_owned(),
            "candid".to_owned(),
            String::new(),
            "200".to_owned(),
        ];
        assert_eq!(shell_command(&argv), "redis:logs candid '' 200");
    }

    #[test]
    fn shell_command_keeps_mount_specs_bare() {
        let argv = vec![
            "storage:mount".to_owned(),
            "myapp".to_owned(),
            "/var/lib/dokku/data/storage/myapp:/app/storage:ro,Z".to_owned(),
        ];
        assert_eq!(
            shell_command(&argv),
            "storage:mount myapp /var/lib/dokku/data/storage/myapp:/app/storage:ro,Z"
        );
    }

    #[test]
    fn shell_command_quotes_and_escapes_unsafe_arguments() {
        assert_eq!(quote_arg("two words"), "'two words'");
        assert_eq!(quote_arg("it's"), r#"'it'\''s'"#);
        assert_eq!(quote_arg("$HOME"), "'$HOME'");
    }
}
