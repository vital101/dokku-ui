use std::time::{Duration, Instant};

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

/// How often an idle live stream writes a keepalive frame. This is what makes
/// a dead browser connection visible to actix (and keeps proxies from timing
/// out the stream) when the app is quiet and produces no log output.
const SSE_KEEPALIVE: Duration = Duration::from_secs(10);

#[derive(serde::Deserialize)]
pub struct StreamQuery {
    #[serde(default)]
    source: String,
    #[serde(default = "default_tail")]
    tail: u8,
    #[serde(default)]
    process: Option<String>,
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
    // The process filter only applies to the app's own log source.
    let process = match query.process.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(process) if source == CapabilityFamily::Logs => {
            if !crate::domain::resource::is_valid_process_type(process) {
                return Err(AppError::BadRequest("invalid process type".into()));
            }
            Some(process.to_owned())
        }
        Some(_) => None,
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

    let command = log_command(source, app, query.tail != 0, process);
    let (tx, rx) = mpsc::channel::<String>(64);
    let client = state.dokku.clone();
    let task = tokio::spawn(async move {
        let _ = client.exec_streaming(&command, tx).await;
    });

    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-store"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(live_stream(rx, task, stream_max_lifetime(&state))))
}

/// The configured live-tail lifetime; `0` disables the cap.
pub(super) fn stream_max_lifetime(state: &AppState) -> Option<Duration> {
    (state.settings.log_stream_max_secs > 0)
        .then(|| Duration::from_secs(state.settings.log_stream_max_secs))
}

fn log_command(
    source: CapabilityFamily,
    app: AppName,
    follow: bool,
    process: Option<String>,
) -> DokkuCommand {
    match source {
        CapabilityFamily::Logs => DokkuCommand::Logs {
            app,
            num_lines: 200,
            follow,
            process,
        },
        CapabilityFamily::NginxAccessLogs => DokkuCommand::NginxAccessLogs { app, follow },
        CapabilityFamily::NginxErrorLogs => DokkuCommand::NginxErrorLogs { app, follow },
    }
}

struct StreamState {
    rx: mpsc::Receiver<String>,
    task: Option<tokio::task::JoinHandle<()>>,
    last_emit: Instant,
    started: Instant,
    keepalive: Duration,
    max_lifetime: Option<Duration>,
    ending: bool,
}

/// Forwards output chunks as SSE `line` events; the stream ends when the
/// command finishes. Dropping the returned stream drops `rx`, which makes the
/// client's next send fail and aborts the SSH channel.
///
/// Idle streams emit a keepalive comment so dead viewers are noticed, and a
/// live tail is capped at `max_lifetime`: it emits `event: end` (the browser
/// reconnects) and then drops `rx` so the remote `docker logs --follow` is
/// closed even when the tab was lost rather than closed.
pub(super) fn live_stream(
    rx: mpsc::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
    max_lifetime: Option<Duration>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    live_stream_with(rx, task, SSE_KEEPALIVE, max_lifetime)
}

fn live_stream_with(
    rx: mpsc::Receiver<String>,
    task: tokio::task::JoinHandle<()>,
    keepalive: Duration,
    max_lifetime: Option<Duration>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    unfold(
        StreamState {
            rx,
            task: Some(task),
            last_emit: Instant::now(),
            started: Instant::now(),
            keepalive,
            max_lifetime,
            ending: false,
        },
        |mut state| async move {
            loop {
                if state.ending {
                    state.task.take();
                    return None;
                }
                if state
                    .max_lifetime
                    .is_some_and(|max| state.started.elapsed() >= max)
                {
                    state.ending = true;
                    return Some((
                        Ok(Bytes::from_static(b"event: end\ndata: reconnect\n\n")),
                        state,
                    ));
                }
                let keepalive_wait = state.keepalive.saturating_sub(state.last_emit.elapsed());
                let until_max = state
                    .max_lifetime
                    .map(|max| max.saturating_sub(state.started.elapsed()))
                    .unwrap_or(Duration::MAX);
                match tokio::time::timeout(keepalive_wait.min(until_max), state.rx.recv()).await {
                    Ok(Some(chunk)) => {
                        state.last_emit = Instant::now();
                        return Some((Ok(line_events(&chunk)), state));
                    }
                    Ok(None) => {
                        state.task.take();
                        return None;
                    }
                    Err(_) => {
                        state.last_emit = Instant::now();
                        if state
                            .max_lifetime
                            .is_some_and(|max| state.started.elapsed() >= max)
                        {
                            continue;
                        }
                        return Some((Ok(Bytes::from_static(b": keepalive\n\n")), state));
                    }
                }
            }
        },
    )
}

pub(super) fn line_events(chunk: &str) -> Bytes {
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
    use futures_util::StreamExt;

    use super::*;

    #[tokio::test]
    async fn idle_stream_emits_keepalives_then_ends_at_its_lifetime() {
        let (tx, rx) = mpsc::channel::<String>(4);
        let task = tokio::spawn(async move {
            let _tx = tx;
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let mut stream = Box::pin(live_stream_with(
            rx,
            task,
            Duration::from_millis(10),
            Some(Duration::from_millis(50)),
        ));

        let mut saw_keepalive = false;
        let end = loop {
            let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
                .await
                .expect("stream makes progress")
                .expect("stream yields")
                .expect("no io error");
            if item == Bytes::from_static(b": keepalive\n\n") {
                saw_keepalive = true;
                continue;
            }
            break item;
        };
        assert!(saw_keepalive, "idle streams heartbeat");
        assert_eq!(end, Bytes::from_static(b"event: end\ndata: reconnect\n\n"));
        assert!(
            tokio::time::timeout(Duration::from_secs(1), stream.next())
                .await
                .expect("stream ends")
                .is_none()
        );
    }

    #[tokio::test]
    async fn dropping_the_stream_closes_the_sink() {
        let (tx, rx) = mpsc::channel::<String>(4);
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            tx.closed().await;
            let _ = closed_tx.send(());
        });
        let stream = live_stream_with(
            rx,
            task,
            Duration::from_secs(60),
            Some(Duration::from_secs(60)),
        );

        drop(stream);
        tokio::time::timeout(Duration::from_secs(1), closed_rx)
            .await
            .expect("the channel observes the drop")
            .expect("sender task finished");
    }

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
            log_command(CapabilityFamily::Logs, app("alpha"), true, None),
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 200,
                follow: true,
                process: None
            }
        );
        assert_eq!(
            log_command(CapabilityFamily::NginxAccessLogs, app("alpha"), true, None),
            DokkuCommand::NginxAccessLogs {
                app: app("alpha"),
                follow: true,
            }
        );
    }

    #[test]
    fn app_log_streams_carry_the_process_filter() {
        assert_eq!(
            log_command(
                CapabilityFamily::Logs,
                app("alpha"),
                true,
                Some("web".into())
            ),
            DokkuCommand::Logs {
                app: app("alpha"),
                num_lines: 200,
                follow: true,
                process: Some("web".into()),
            }
        );
    }

    fn app(name: &str) -> AppName {
        AppName::try_from(name).expect("valid app name")
    }
}
