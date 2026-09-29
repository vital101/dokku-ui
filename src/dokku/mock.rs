use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::domain::command::DokkuCommand;

use super::client::{DokkuClient, DokkuError, DokkuOutput};

#[derive(Debug)]
pub struct MockClient {
    responses: Mutex<HashMap<DokkuCommand, Result<DokkuOutput, DokkuError>>>,
    default: Result<DokkuOutput, DokkuError>,
    calls: Mutex<Vec<DokkuCommand>>,
}

impl Default for MockClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClient {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            default: Ok(DokkuOutput::ok("")),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn with_default(default: Result<DokkuOutput, DokkuError>) -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            default,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn stub(self, command: DokkuCommand, output: Result<DokkuOutput, DokkuError>) -> Self {
        self.responses
            .lock()
            .expect("responses lock")
            .insert(command, output);
        self
    }

    pub fn calls(&self) -> Vec<DokkuCommand> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait]
impl DokkuClient for MockClient {
    async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
        self.calls.lock().expect("calls lock").push(command.clone());
        let response = self
            .responses
            .lock()
            .expect("responses lock")
            .get(command)
            .cloned()
            .unwrap_or_else(|| self.default.clone());
        match response {
            Ok(output) if output.exit_code != 0 => Err(DokkuError::Exit {
                code: output.exit_code,
                stderr: output.stderr,
            }),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::AppName;

    use super::*;

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    fn exit_error(code: i32, stderr: &str) -> DokkuError {
        DokkuError::Exit {
            code,
            stderr: stderr.to_owned(),
        }
    }

    #[tokio::test]
    async fn returns_stubbed_output_and_records_call() {
        let command = DokkuCommand::AppsList;
        let client = MockClient::new().stub(command.clone(), Ok(DokkuOutput::ok("[\"a\"]")));

        let output = client.exec(&command).await.expect("stubbed output");
        assert_eq!(output.stdout, "[\"a\"]");
        assert_eq!(client.calls(), vec![command]);
    }

    #[tokio::test]
    async fn falls_back_to_default_for_unstubbed_commands() {
        let client = MockClient::new();
        let output = client.exec(&DokkuCommand::AppsList).await.expect("default");
        assert_eq!(output, DokkuOutput::ok(""));
    }

    #[tokio::test]
    async fn returns_default_error_when_set() {
        let client = MockClient::with_default(Err(exit_error(1, "boom")));
        let err = client
            .exec(&DokkuCommand::AppsList)
            .await
            .expect_err("error");
        assert_eq!(err, exit_error(1, "boom"));
    }

    #[tokio::test]
    async fn converts_non_zero_exit_to_error() {
        let command = DokkuCommand::PsStart { app: app("myapp") };
        let client = MockClient::new().stub(
            command.clone(),
            Ok(DokkuOutput {
                exit_code: 1,
                stdout: String::new(),
                stderr: "app not found".into(),
            }),
        );

        let err = client.exec(&command).await.expect_err("exit error");
        assert_eq!(err, exit_error(1, "app not found"));
    }

    #[tokio::test]
    async fn records_calls_in_order() {
        let client = MockClient::new();
        let list = DokkuCommand::AppsList;
        let report = DokkuCommand::PsReport { app: app("myapp") };
        let _ = client.exec(&list).await;
        let _ = client.exec(&report).await;
        assert_eq!(client.calls(), vec![list, report]);
    }

    #[tokio::test]
    async fn works_behind_the_trait() {
        let client: Box<dyn DokkuClient> = Box::new(MockClient::new());
        let output = client.exec(&DokkuCommand::AppsList).await.expect("default");
        assert_eq!(output.exit_code, 0);
    }
}
