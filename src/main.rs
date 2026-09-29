use std::io;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("dokku_ui=info,actix_web=info")),
        )
        .init();

    let settings = dokku_ui::settings::Settings::from_env().map_err(io::Error::other)?;
    dokku_ui::run(settings).await
}
