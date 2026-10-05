use std::sync::Arc;
use std::time::{Duration, Instant};

use actix_session::Session;
use actix_web::web::Bytes;
use actix_web::{HttpResponse, web};
use futures_util::Stream;
use futures_util::stream::unfold;

use crate::error::AppError;
use crate::storage::runs::{ORPHAN_AFTER_SECS, RunOutcome, SqliteRunsRepo, interrupted_outcome};
use crate::web::fragments::current_user;
use crate::web::state::AppState;

const SSE_KEEPALIVE: Duration = Duration::from_secs(10);
const POLL_MIN: Duration = Duration::from_millis(25);
const POLL_MAX: Duration = Duration::from_millis(400);

/// Streams any registered action run by id. Every run (app, service, volume)
/// uses this one route; access is gated by the auth middleware. Runs live in
/// SQLite, so any process can serve the stream — not just the one that
/// started the run — which is what makes `ps:scale web=N` safe.
pub async fn action_events(
    state: web::Data<AppState>,
    session: Session,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let id = path.into_inner();
    if !is_valid_run_id(&id) {
        return Err(AppError::NotFound);
    }
    current_user(&state, &session).await?;
    state
        .action_runs
        .get(&id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-store"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(action_stream(state.action_runs.clone(), id)))
}

/// Run ids are 64 lowercase hex characters (`auth::csrf::generate_token`).
fn is_valid_run_id(id: &str) -> bool {
    id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit())
}

struct StreamState {
    repo: Arc<SqliteRunsRepo>,
    run_id: String,
    cursor: i64,
    backoff: Duration,
    last_emit: Instant,
    done: bool,
}

/// Polls SQLite for new lines and the terminal outcome, emitting the same
/// `line`/`done` events as before. The poll cadence backs off from 25ms to
/// 400ms while the run is quiet, so a finished run replays almost instantly
/// and a long build costs little. If the run goes silent past the orphan
/// window (its owner process died), a synthetic failure is delivered.
fn action_stream(
    repo: Arc<SqliteRunsRepo>,
    run_id: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    unfold(
        StreamState {
            repo,
            run_id,
            cursor: -1,
            backoff: POLL_MIN,
            last_emit: Instant::now(),
            done: false,
        },
        |mut state| async move {
            if state.done {
                return None;
            }
            loop {
                match state.repo.get(&state.run_id).await {
                    Ok(Some(row)) => {
                        let lines = match state.repo.lines_after(&state.run_id, state.cursor).await
                        {
                            Ok(lines) => lines,
                            Err(err) => {
                                tracing::warn!(error = %err, "run line poll failed; closing stream");
                                return None;
                            }
                        };
                        if !lines.is_empty() {
                            state.cursor =
                                lines.last().map(|(seq, _)| *seq).unwrap_or(state.cursor);
                            state.backoff = POLL_MIN;
                            state.last_emit = Instant::now();
                            let events = line_events(
                                &lines.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
                            );
                            return Some((Ok(events), state));
                        }
                        if let Some(outcome) = row.outcome {
                            state.done = true;
                            return Some((Ok(done_event(&outcome)), state));
                        }
                        if row.updated_at < now_epoch() - ORPHAN_AFTER_SECS {
                            let outcome = interrupted_outcome();
                            let _ = state.repo.finish(&state.run_id, &outcome).await;
                            state.done = true;
                            return Some((Ok(done_event(&outcome)), state));
                        }
                    }
                    Ok(None) => {
                        let outcome = RunOutcome {
                            ok: false,
                            message: "Run expired before it could complete.".to_owned(),
                            redirect: None,
                        };
                        state.done = true;
                        return Some((Ok(done_event(&outcome)), state));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "run poll failed; closing stream");
                        return None;
                    }
                }
                if state.last_emit.elapsed() >= SSE_KEEPALIVE {
                    state.last_emit = Instant::now();
                    return Some((Ok(Bytes::from_static(b": keepalive\n\n")), state));
                }
                tokio::time::sleep(state.backoff).await;
                if state.backoff < POLL_MAX {
                    state.backoff = state.backoff.saturating_mul(2).min(POLL_MAX);
                }
            }
        },
    )
}

fn now_epoch() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_64_hex_characters() {
        assert!(is_valid_run_id(&"a".repeat(64)));
        assert!(!is_valid_run_id(&"g".repeat(64)), "g is not hex");
        assert!(!is_valid_run_id(&"a".repeat(63)));
        assert!(!is_valid_run_id(""));
    }

    #[test]
    fn line_events_frame_each_line() {
        let events = line_events(&["one".to_owned(), "two".to_owned()]);
        assert_eq!(
            events,
            Bytes::from("event: line\ndata: one\n\nevent: line\ndata: two\n\n")
        );
    }

    #[test]
    fn done_event_serializes_the_outcome() {
        let outcome = RunOutcome {
            ok: true,
            message: "done".to_owned(),
            redirect: Some("/apps/alpha".to_owned()),
        };
        let event = done_event(&outcome);
        let text = String::from_utf8(event.to_vec()).expect("utf8");
        assert!(text.starts_with("event: done\ndata: "));
        assert!(text.contains("\"ok\":true"));
        assert!(text.contains("\"redirect\":\"/apps/alpha\""));
    }
}
