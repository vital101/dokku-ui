use std::path::PathBuf;

use async_trait::async_trait;

use crate::domain::command::DokkuCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DokkuOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl DokkuOutput {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DokkuError {
    #[error("failed to connect to dokku host: {0}")]
    Connect(String),
    #[error("ssh authentication failed")]
    AuthFailed,
    #[error("failed to read ssh key at `{path}`: {error}")]
    KeyLoad { path: PathBuf, error: String },
    #[error("failed to parse ssh key: {0}")]
    KeyParse(String),
    #[error("command timed out after {secs}s")]
    Timeout { secs: u64 },
    #[error("dokku command failed (exit {code}): {stderr}")]
    Exit { code: i32, stderr: String },
}

#[async_trait]
pub trait DokkuClient: Send + Sync {
    async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError>;
}
