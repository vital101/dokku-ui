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

    let port = std::env::var("PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8080);

    dokku_ui::run(port).await
}
