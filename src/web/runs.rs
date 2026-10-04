use std::sync::Arc;
use std::time::Duration;

use actix_session::Session;
use actix_web::web::Bytes;
use actix_web::{HttpResponse, web};
use futures_util::Stream;
use futures_util::stream::unfold;

use crate::dokku::{ActionRun, RunOutcome};
use crate::error::AppError;
use crate::web::fragments::current_user;
use crate::web::state::AppState;

const SSE_KEEPALIVE: Duration = Duration::from_secs(10);

/// Streams any registered action run by id. Every run (app, service, volume)
/// uses this one route; access is gated by the auth middleware.
pub async fn action_events(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<u64>,
) -> Result<HttpResponse, AppError> {
    let id = path.into_inner();
    current_user(&state, &session).await?;

    let run = state.action_runs.get(id).await.ok_or(AppError::NotFound)?;

    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-store"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(action_stream(run)))
}

/// Replays buffered output, then follows the run until it finishes. Emits
/// `line` events for output and one `done` event carrying the JSON outcome.
fn action_stream(run: Arc<ActionRun>) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    let follower = run.subscribe();
    unfold(
        (run, 0usize, follower, false),
        |(run, mut cursor, mut follower, done_sent)| async move {
            if done_sent {
                return None;
            }
            loop {
                let (lines, outcome) = run.poll(&mut cursor).await;
                if !lines.is_empty() {
                    return Some((Ok(line_events(&lines)), (run, cursor, follower, false)));
                }
                if let Some(outcome) = outcome {
                    return Some((Ok(done_event(&outcome)), (run, cursor, follower, true)));
                }
                match tokio::time::timeout(SSE_KEEPALIVE, follower.changed()).await {
                    Ok(Ok(())) => continue,
                    Ok(Err(_)) => return None,
                    Err(_) => {
                        return Some((
                            Ok(Bytes::from_static(b": keepalive\n\n")),
                            (run, cursor, follower, false),
                        ));
                    }
                }
            }
        },
    )
}

fn line_events(lines: &[String]) -> Bytes {
    let mut body = String::new();
    for line in lines {
        body.push_str("event: line\ndata: ");
        body.push_str(line);
        body.push_str("\n\n");
    }
    Bytes::from(body)
}

fn done_event(outcome: &RunOutcome) -> Bytes {
    let data = serde_json::to_string(outcome).unwrap_or_else(|_| {
        r#"{"ok":false,"message":"failed to serialize outcome","redirect":null}"#.to_owned()
    });
    Bytes::from(format!("event: done\ndata: {data}\n\n"))
}
