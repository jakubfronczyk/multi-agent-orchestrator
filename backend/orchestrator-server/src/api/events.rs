//! `GET /api/events`: every committed run event, live, as server-sent events.
//! A subscriber that falls behind the broadcast buffer has its stream ended and reconnects.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use super::AppState;

const KEEP_ALIVE: Duration = Duration::from_secs(15);

pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = BroadcastStream::new(state.store.subscribe())
        .map_while(Result::ok)
        .filter_map(|envelope| {
            Event::default()
                .id(envelope.id.0.to_string())
                .json_data(&envelope)
                .ok()
        })
        .map(Ok);
    Sse::new(events).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}
