/// Minimal SSE line splitter: feeds bytes in, yields `data:` payloads.
/// Handles the three flavours we consume (OpenAI-compat, Anthropic, Gemini).
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(8 * 1024) }
    }

    /// Events from this chunk. `done` is set when the stream sends `data: [DONE]`,
    /// which is a clean end — the caller should stop reading so a following
    /// socket close is not reported as a body-decode failure.
    pub fn push(&mut self, bytes: &[u8]) -> SseBatch {
        let mut out = Vec::new();
        self.buf.extend_from_slice(bytes);
        while let Some(idx) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=idx).collect();
            let line = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string();
            if line.is_empty() || line.starts_with(':') || line.starts_with("event:") || line.starts_with("id:") {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim_start();
                if data == "[DONE]" {
                    return SseBatch { data: out, done: true };
                }
                out.push(data.to_string());
            }
        }
        SseBatch { data: out, done: false }
    }
}

pub struct SseBatch {
    pub data: Vec<String>,
    pub done: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_ends_the_batch_after_the_last_event() {
        let mut p = SseParser::new();
        let batch = p.push(b"data: {\"a\":1}\n\ndata: [DONE]\n");
        assert_eq!(batch.data, vec!["{\"a\":1}"]);
        assert!(batch.done);
    }
}
