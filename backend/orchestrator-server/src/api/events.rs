//! `GET /api/events`: committed run events as server-sent events. Without `Last-Event-ID` the
//! stream is live only; with it, every event after that id is replayed first, then live follows.
//! A subscriber that falls behind the broadcast buffer has its stream ended and reconnects.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use orchestrator_core::ids::EventId;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use super::AppState;
use super::error::ApiError;
use crate::events::EventEnvelope;
use crate::sqlite::events;

const KEEP_ALIVE: Duration = Duration::from_secs(15);
const LAST_EVENT_ID: &str = "last-event-id";

/// Subscribes before reading the log, so an event committed during the replay is either in the
/// replay or in the live stream; the live stream skips ids the replay already sent.
pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let live = BroadcastStream::new(state.store.subscribe()).map_while(Result::ok);
    // An unreadable id cannot name a replay point, so it is treated as absent: live only.
    let after = headers
        .get(LAST_EVENT_ID)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .map(EventId);
    let after = match after {
        Some(after) => {
            // An id above the newest one comes from before a database reset: live only, or the
            // filter would hide every new event until the ids caught up.
            let newest = events::last_id(&state.pool).await.map_err(replay_failed)?;
            newest.filter(|newest| after <= *newest).map(|_| after)
        }
        None => None,
    };
    let (replayed, sent_up_to) = match after {
        Some(after) => {
            let replayed = events::since(&state.pool, after)
                .await
                .map_err(replay_failed)?;
            let last = replayed.last().map_or(after, |envelope| envelope.id);
            (replayed, Some(last))
        }
        None => (Vec::new(), None),
    };
    let live = live.filter(move |envelope| sent_up_to.is_none_or(|last| envelope.id > last));
    let events = tokio_stream::iter(replayed)
        .chain(live)
        .filter_map(|envelope| frame(&envelope))
        .map(Ok);
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(KEEP_ALIVE)))
}

fn replay_failed(error: events::EventReadError) -> ApiError {
    tracing::error!(%error, "event replay failed");
    ApiError::internal()
}

fn frame(envelope: &EventEnvelope) -> Option<Event> {
    Event::default()
        .id(envelope.id.0.to_string())
        .json_data(envelope)
        .ok()
}
