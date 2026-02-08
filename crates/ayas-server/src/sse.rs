use axum::response::sse::{Event, KeepAlive};
use futures::Stream;
use serde::Serialize;

/// Create an SSE response from a stream of events with keep-alive.
pub fn sse_response<S>(
    stream: S,
) -> axum::response::Sse<axum::response::sse::KeepAliveStream<S>>
where
    S: Stream<Item = Result<Event, std::convert::Infallible>> + Send + 'static,
{
    axum::response::Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Create an SSE Event from a serializable value.
pub fn sse_event<T: Serialize>(data: &T) -> Result<Event, std::convert::Infallible> {
    let json = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
    Ok(Event::default().data(json))
}

/// Create an SSE done event.
pub fn sse_done() -> Result<Event, std::convert::Infallible> {
    Ok(Event::default().data("[DONE]"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentSseEvent;

    #[test]
    fn sse_event_serializes_json() {
        let event = AgentSseEvent::Message {
            content: "Hello".into(),
        };
        let result = sse_event(&event);
        assert!(result.is_ok());
    }

    #[test]
    fn sse_done_event() {
        let result = sse_done();
        assert!(result.is_ok());
    }
}
