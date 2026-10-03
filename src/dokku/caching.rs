use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::domain::AppName;
use crate::domain::command::DokkuCommand;

use super::client::{DokkuClient, DokkuError, DokkuOutput};

#[derive(Debug, thiserror::Error)]
#[error("cached dokku client requires a ttl")]
pub struct InvalidTtl;

pub struct CachingDokkuClient {
    inner: Arc<dyn DokkuClient>,
    ttl: Duration,
    reports: tokio::sync::Mutex<HashMap<AppName, (Instant, DokkuOutput)>>,
}

impl CachingDokkuClient {
    pub fn new(inner: Arc<dyn DokkuClient>, ttl: Duration) -> Result<Self, InvalidTtl> {
        if ttl.is_zero() {
            return Err(InvalidTtl);
        }
        Ok(Self {
            inner,
            ttl,
            reports: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    fn invalidated_app(command: &DokkuCommand) -> Option<&AppName> {
        match command {
            DokkuCommand::PsStart { app }
            | DokkuCommand::PsStop { app }
            | DokkuCommand::PsRestart { app }
            | DokkuCommand::AppsDestroy { app, .. } => Some(app),
            DokkuCommand::PsReport { .. }
            | DokkuCommand::AppsList
            | DokkuCommand::AppsCreate { .. }
            | DokkuCommand::AppsReport { .. }
            | DokkuCommand::ConfigShow { .. }
            | DokkuCommand::Logs { .. } => None,
        }
    }
}

#[async_trait]
impl DokkuClient for CachingDokkuClient {
    async fn exec(&self, command: &DokkuCommand) -> Result<DokkuOutput, DokkuError> {
        if let DokkuCommand::PsReport { app } = command {
            let cached = self.reports.lock().await.get(app).cloned();
            if let Some((fetched_at, output)) = cached {
                if fetched_at.elapsed() < self.ttl {
                    return Ok(output);
                }
            }
            let output = self.inner.exec(command).await?;
            self.reports
                .lock()
                .await
                .insert(app.clone(), (Instant::now(), output.clone()));
            return Ok(output);
        }

        let output = self.inner.exec(command).await?;
        if let Some(app) = Self::invalidated_app(command) {
            self.reports.lock().await.remove(app);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dokku::MockClient;

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }

    fn report_output(processes: i64) -> DokkuOutput {
        DokkuOutput::ok(format!(
            r#"{{"deployed": "true", "running": "true", "processes": "{processes}"}}"#
        ))
    }

    fn cache(inner: Arc<MockClient>, ttl: Duration) -> CachingDokkuClient {
        CachingDokkuClient::new(inner, ttl).expect("client")
    }

    fn count(inner: &Arc<MockClient>, command: &DokkuCommand) -> usize {
        inner.calls().iter().filter(|c| c == &command).count()
    }

    #[test]
    fn rejects_zero_ttl() {
        let inner: Arc<dyn DokkuClient> = Arc::new(MockClient::new());
        assert!(CachingDokkuClient::new(inner, Duration::ZERO).is_err());
    }

    #[tokio::test]
    async fn repeated_ps_reports_fetch_once() {
        let command = DokkuCommand::PsReport { app: app("alpha") };
        let inner = Arc::new(MockClient::new().stub(command.clone(), Ok(report_output(1))));
        let client = cache(inner.clone(), Duration::from_secs(30));

        let first = client.exec(&command).await.expect("first");
        let second = client.exec(&command).await.expect("second");
        assert_eq!(first.stdout, second.stdout);
        assert_eq!(count(&inner, &command), 1);
    }

    #[tokio::test]
    async fn ps_reports_are_cached_per_app() {
        let alpha = DokkuCommand::PsReport { app: app("alpha") };
        let beta = DokkuCommand::PsReport { app: app("beta") };
        let inner = Arc::new(
            MockClient::new()
                .stub(alpha.clone(), Ok(report_output(1)))
                .stub(beta.clone(), Ok(report_output(2))),
        );
        let client = cache(inner.clone(), Duration::from_secs(30));

        for _ in 0..3 {
            let a = client.exec(&alpha).await.expect("alpha");
            let b = client.exec(&beta).await.expect("beta");
            assert!(a.stdout.contains(r#""processes": "1""#));
            assert!(b.stdout.contains(r#""processes": "2""#));
        }
        assert_eq!(count(&inner, &alpha), 1);
        assert_eq!(count(&inner, &beta), 1);
    }

    #[tokio::test]
    async fn mutating_actions_invalidate_only_their_app() {
        let alpha = DokkuCommand::PsReport { app: app("alpha") };
        let beta = DokkuCommand::PsReport { app: app("beta") };
        let inner = Arc::new(MockClient::new());
        let client = cache(inner.clone(), Duration::from_secs(60));

        client.exec(&alpha).await.expect("report alpha");
        client
            .exec(&DokkuCommand::PsRestart { app: app("alpha") })
            .await
            .expect("restart");
        client.exec(&alpha).await.expect("refetch alpha");
        assert_eq!(count(&inner, &alpha), 2, "restart invalidated alpha");

        client.exec(&beta).await.expect("report beta");
        client
            .exec(&DokkuCommand::PsStop { app: app("alpha") })
            .await
            .expect("stop alpha");
        client.exec(&beta).await.expect("beta still cached");
        assert_eq!(count(&inner, &beta), 1, "beta unaffected");
    }

    #[tokio::test]
    async fn destroy_invalidates_the_report() {
        let alpha = DokkuCommand::PsReport { app: app("alpha") };
        let inner = Arc::new(MockClient::new());
        let client = cache(inner.clone(), Duration::from_secs(60));

        client.exec(&alpha).await.expect("report");
        client
            .exec(&DokkuCommand::AppsDestroy {
                app: app("alpha"),
                force: true,
            })
            .await
            .expect("destroy");
        client.exec(&alpha).await.expect("refetch");
        assert_eq!(count(&inner, &alpha), 2);
    }

    #[tokio::test]
    async fn expired_entries_refetch() {
        let alpha = DokkuCommand::PsReport { app: app("alpha") };
        let inner = Arc::new(MockClient::new());
        let client = cache(inner.clone(), Duration::from_millis(40));

        client.exec(&alpha).await.expect("report");
        tokio::time::sleep(Duration::from_millis(60)).await;
        client.exec(&alpha).await.expect("refetch");
        assert_eq!(count(&inner, &alpha), 2);
    }

    #[tokio::test]
    async fn failed_reports_are_not_cached() {
        let alpha = DokkuCommand::PsReport { app: app("alpha") };
        let inner = Arc::new(
            MockClient::new().stub(alpha.clone(), Err(DokkuError::Connect("refused".into()))),
        );
        let client = cache(inner.clone(), Duration::from_secs(60));

        for _ in 0..2 {
            assert!(client.exec(&alpha).await.is_err());
        }
        assert_eq!(count(&inner, &alpha), 2, "errors retried, not cached");
    }

    #[tokio::test]
    async fn read_only_commands_pass_through_uncached() {
        let inner = Arc::new(MockClient::new());
        let client = cache(inner.clone(), Duration::from_secs(60));

        for _ in 0..3 {
            client.exec(&DokkuCommand::AppsList).await.expect("list");
        }
        assert_eq!(count(&inner, &DokkuCommand::AppsList), 3);
    }
}
