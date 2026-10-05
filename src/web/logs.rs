use actix_session::Session;
use actix_web::web::Bytes;
use actix_web::{HttpResponse, web};
use futures_util::Stream;
use futures_util::stream::unfold;
use tokio::sync::mpsc;

use crate::domain::AppName;
use crate::domain::capabilities::CapabilityFamily;
use crate::domain::command::DokkuCommand;
use crate::error::AppError;
use crate::web::fragments::current_user;
use crate::web::state::AppState;

#[derive(serde::Deserialize)]
pub struct StreamQuery {
    #[serde(default)]
    source: String,
    #[serde(default = "default_tail")]
    tail: u8,
}

fn default_tail() -> u8 {
    1
}

/// Streams a live log source over SSE. Each viewer owns one SSH channel;
/// when the browser disconnects the response stream drops, the mpsc receiver
/// drops, and `RusshClient` aborts the channel (`DokkuError::StreamClosed`) —
/// no lingering tail processes, and the shared session is unharmed.
pub async fn log_stream(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
    query: web::Query<StreamQuery>,
) -> Result<HttpResponse, AppError> {
    let name = path.into_inner();
    current_user(&state, &session).await?;
    let app = AppName::try_from(name.as_str()).map_err(|_| AppError::NotFound)?;
    let source = match CapabilityFamily::try_from(query.source.as_str()) {
        Ok(source) if source != CapabilityFamily::Logs => source,
        _ => CapabilityFamily::Logs,
    };

    // Capability-gate each source independently: an unsupported family sends
    // an explanatory SSE error instead of a failed SSH attempt.
    if query.tail != 0 {
        if let Some(caps) = state.capabilities.current().await {
            let support = caps.supports_family(source);
            if support != crate::domain::capabilities::Support::Supported {
                let message = support.label();
                let event = Bytes::from(format!("event: error\ndata: {message}\n\n"));
                let chunk: Result<Bytes, std::io::Error> = Ok(event);
                return Ok(HttpResponse::Ok()
                    .content_type("text/event-stream")
                    .insert_header(("Cache-Control", "no-store"))
                    .insert_header(("X-Accel-Buffering", "no"))
                    .streaming(futures_util::stream::iter(vec![chunk])));
            }
        }
    }

    let command = log_command(source, app, query.tail != 0);
    let (tx, rx) = mpsc::channel::<String>(64);
    let client = state.dokku.clone();
    let task = tokio::spawn(async move {
        let _ = client.exec_streaming(&command, tx).await;
    });

    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-store"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(live_stream(rx, task)))
}

fn log_command(source: CapabilityFamily, app: AppName, follow: bool) -> DokkuCommand {
    match source {
        CapabilityFamily::Logs => DokkuCommand::Logs {
            app,
            num_lines: 200,
            follow,
        },
        CapabilityFamily::NginxAccessLogs => DokkuCommand::NginxAccessLogs { app, follow },
        CapabilityFamily::NginxErrorLogs => DokkuCommand::NginxErrorLogs { app, follow },
    }
}

struct StreamState {
    rx: mpsc::Receiver<String>,
    task: Option<tokio::task::JoinHandle<()>>,
    last_emit: std::time::Instant,
}

/// Forwards output chunks as SSE `line` events; the stream ends when the
/// command finishes. Dropping the returned stream drops `rx`, which makes the
/// client's next send fail and aborts the SSH channel.
fn live_stream(
    rx: mpsc::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    unfold(
        StreamState {
            rx,
            task: Some(task),
            last_emit: std::time::Instant::now(),
        },
        |mut state| async move {
            match state.rx.recv().await {
                Some(chunk) => {
                    state.last_emit = std::time::Instant::now();
                    Some((Ok(line_events(&chunk)), state))
                }
                None => {
                    state.task.take();
                    None
                }
            }
        },
    )
}

fn line_events(chunk: &str) -> Bytes {
    let mut body = String::new();
    for line in chunk.lines() {
        if line.is_empty() {
            continue;
        }
        body.push_str("event: line\ndata: ");
        body.push_str(line);
        body.push_str("\n\n");
    }
    if body.is_empty() {
        Bytes::new()
    } else {
        Bytes::from(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_events_frame_each_line() {
        let events = line_events("one\ntwo\n");
        assert_eq!(
            events,
            Bytes::from("event: line\ndata: one\n\nevent: line\ndata: two\n\n")
        );
        assert_eq!(line_events(""), Bytes::new());
        assert_eq!(line_events("\n"), Bytes::new());
    }

    #[test]
    fn unknown_sources_fall_back_to_app_logs() {
        assert_eq!(
            log_command(CapabilityFamily::Logs, app("alpha"), true),
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 200,
                follow: true,
            }
        );
        assert_eq!(
            log_command(CapabilityFamily::NginxAccessLogs, app("alpha"), true),
            DokkuCommand::NginxAccessLogs {
                app: app("alpha"),
                follow: true,
            }
        );
    }

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }
}
