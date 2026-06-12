//! Incremental SSE parser for the bridge `/sse` stream.
//!
//! Copies the fuller in-tree Pattern B idiom (`context_server` transport
//! `http.rs::setup_sse_stream`: multi-line data buffering, blank-line event
//! termination, field filtering) but as a pure chunk-feed state machine so it
//! is unit-testable on partial-chunk fixtures and owns no I/O. The client
//! reads raw bytes off the response body and feeds them here; complete event
//! payloads (joined `data:` lines) come back out.

/// Byte-fed SSE event assembler. Survives chunk boundaries anywhere —
/// mid-line, mid-UTF-8 — because it only interprets complete `\n`-terminated
/// lines and converts to text per line.
#[derive(Default)]
pub struct SseParser {
    /// Unterminated tail of the byte stream (a partial line).
    line_buffer: Vec<u8>,
    /// `data:` lines of the event currently being assembled.
    data_lines: Vec<String>,
}

impl SseParser {
    /// Feed a raw chunk; returns the data payloads of every event completed
    /// by it (an event completes on its blank line).
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        for byte in chunk {
            if *byte != b'\n' {
                self.line_buffer.push(*byte);
                continue;
            }
            let mut line = std::mem::take(&mut self.line_buffer);
            if line.last() == Some(&b'\r') {
                line.pop(); // CRLF tolerance
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                // Blank line terminates the event.
                if !self.data_lines.is_empty() {
                    events.push(self.data_lines.join("\n"));
                    self.data_lines.clear();
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data_lines.push(data.strip_prefix(' ').unwrap_or(data).to_string());
            }
            // `event:` / `id:` / `retry:` / `:comment` lines are ignored —
            // the bridge mirrors the type inside the JSON payload (protocol.rs).
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_frame_parses() {
        let mut parser = SseParser::default();
        let events = parser.push(b"event: board\ndata: {\"type\": \"board\", \"board\": []}\n\n");
        assert_eq!(events, vec![r#"{"type": "board", "board": []}"#]);
    }

    #[test]
    fn multi_frame_chunk_parses_in_order() {
        let mut parser = SseParser::default();
        let events = parser.push(
            b"event: board\ndata: {\"type\": \"board\"}\n\n\
              event: usage\ndata: {\"type\": \"usage\"}\n\n",
        );
        assert_eq!(events, vec![r#"{"type": "board"}"#, r#"{"type": "usage"}"#]);
    }

    #[test]
    fn partial_chunks_reassemble() {
        // The frame arrives split mid-field-name, mid-payload, and with the
        // terminating blank line in its own chunk.
        let mut parser = SseParser::default();
        assert!(parser.push(b"event: bo").is_empty());
        assert!(parser.push(b"ard\ndata: {\"type\": ").is_empty());
        assert!(parser.push(b"\"board\"}\n").is_empty());
        let events = parser.push(b"\n");
        assert_eq!(events, vec![r#"{"type": "board"}"#]);
    }

    #[test]
    fn partial_utf8_across_chunks_reassembles() {
        // "—" (U+2014) split between chunks must not be mangled: the parser
        // only converts complete lines.
        let bytes = "data: {\"note\": \"GUARD — hard\"}\n\n".as_bytes();
        let (a, b) = bytes.split_at(17); // splits inside the em-dash bytes
        let mut parser = SseParser::default();
        assert!(parser.push(a).is_empty());
        let events = parser.push(b);
        assert_eq!(events, vec![r#"{"note": "GUARD — hard"}"#]);
    }

    #[test]
    fn multi_line_data_joins_with_newline() {
        let mut parser = SseParser::default();
        let events = parser.push(b"data: line one\ndata: line two\n\n");
        assert_eq!(events, vec!["line one\nline two"]);
    }

    #[test]
    fn non_data_fields_and_comments_are_ignored() {
        let mut parser = SseParser::default();
        let events =
            parser.push(b": keepalive comment\nid: 42\nretry: 3000\ndata: payload\n\n");
        assert_eq!(events, vec!["payload"]);
    }

    #[test]
    fn crlf_lines_parse() {
        let mut parser = SseParser::default();
        let events = parser.push(b"data: payload\r\n\r\n");
        assert_eq!(events, vec!["payload"]);
    }

    #[test]
    fn blank_lines_without_data_emit_nothing() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"\n\n\nevent: ping\n\n").is_empty());
    }

    #[test]
    fn dataless_prefix_variant_parses() {
        // "data:" with no space after the colon (spec-legal).
        let mut parser = SseParser::default();
        let events = parser.push(b"data:tight\n\n");
        assert_eq!(events, vec!["tight"]);
    }
}
