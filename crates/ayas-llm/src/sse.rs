//! Shared SSE (Server-Sent Events) stream parser.

use futures::{Stream, StreamExt};

/// Parse SSE data lines from a reqwest response.
///
/// Buffers incoming bytes, splits by newlines, and yields the payload
/// after each `data: ` prefix.
pub fn sse_data_stream(response: reqwest::Response) -> impl Stream<Item = String> + Send {
    async_stream::stream! {
        let mut buffer = String::new();
        let mut byte_stream = Box::pin(response.bytes_stream());

        while let Some(result) = byte_stream.next().await {
            let chunk = match result {
                Ok(bytes) => bytes,
                Err(_) => break,
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer.drain(..newline_pos + 1);

                if let Some(data) = line.strip_prefix("data: ") {
                    yield data.to_string();
                }
            }
        }

        // Handle any remaining data in the buffer.
        let remaining = buffer.trim_end();
        if let Some(data) = remaining.strip_prefix("data: ") {
            yield data.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    /// Test helper: extract SSE data lines from a raw SSE string.
    pub fn extract_sse_data_lines(raw: &str) -> Vec<String> {
        raw.lines()
            .filter_map(|line| line.strip_prefix("data: ").map(String::from))
            .collect()
    }

    #[test]
    fn extract_data_lines_basic() {
        let raw = "event: message_start\ndata: {\"type\":\"message_start\"}\n\ndata: {\"type\":\"ping\"}\n\n";
        let lines = extract_sse_data_lines(raw);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], r#"{"type":"message_start"}"#);
        assert_eq!(lines[1], r#"{"type":"ping"}"#);
    }

    #[test]
    fn extract_data_lines_done() {
        let raw = "data: [DONE]\n\n";
        let lines = extract_sse_data_lines(raw);
        assert_eq!(lines, vec!["[DONE]"]);
    }

    #[test]
    fn extract_data_lines_empty() {
        let raw = "event: ping\n\n";
        let lines = extract_sse_data_lines(raw);
        assert!(lines.is_empty());
    }
}
