use std::time::Duration;

use dokku_ui::settings::Settings;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn run_serves_healthz_on_configured_port() {
    let dir = TempDir::new().expect("temp dir");
    let database_url = format!("sqlite://{}/run.db", dir.path().display());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);

    let settings = Settings { port, database_url };
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(dokku_ui::run(settings))
    });

    wait_until_healthy(port).await;
    drop(server);
}

async fn wait_until_healthy(port: u16) {
    for _ in 0..100 {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            let request = b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            if stream.write_all(request).await.is_ok() {
                let mut buf = [0; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    if n > 0 && String::from_utf8_lossy(&buf[..n]).contains("200 OK") {
                        return;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become healthy within timeout");
}
