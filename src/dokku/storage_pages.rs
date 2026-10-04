use crate::domain::command::DokkuCommand;
use crate::domain::parse::parse_storage_report;
use crate::domain::types::AppMounts;

use super::client::{DokkuClient, DokkuError};

/// Every app's bind mounts, from the all-apps `storage:report` output. One
/// SSH command covers the whole page; per-app sections with no mounts are
/// still returned so the UI can say "no mounts" distinctly from "unknown".
pub async fn app_mounts(client: &dyn DokkuClient) -> Result<Vec<AppMounts>, DokkuError> {
    let output = client.exec(&DokkuCommand::StorageReport).await?;
    Ok(parse_storage_report(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::{DokkuOutput, MockClient};

    const STORAGE_REPORT: &str = include_str!("../../tests/fixtures/storage_report.txt");

    #[tokio::test]
    async fn app_mounts_parses_fixture() {
        let client = MockClient::new().stub(
            DokkuCommand::StorageReport,
            Ok(DokkuOutput::ok(STORAGE_REPORT)),
        );

        let apps = app_mounts(&client).await.expect("mounts");

        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].app, "dokku-ui");
        assert_eq!(apps[0].mount_count(), 1);
        assert_eq!(apps[1].app, "starwars");
        assert!(apps[1].is_empty());
    }

    #[tokio::test]
    async fn app_mounts_propagates_failure() {
        let client = MockClient::new().stub(
            DokkuCommand::StorageReport,
            Err(DokkuError::Exit {
                code: 1,
                stderr: "boom".into(),
            }),
        );

        assert!(app_mounts(&client).await.is_err());
    }
}
