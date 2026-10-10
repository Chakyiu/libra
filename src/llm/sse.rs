/// Minimal SSE line splitter: feeds bytes in, yields `data:` payloads.
/// Handles the three flavours we consume (OpenAI-compat, Anthropic, Gemini).
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buf: Vec::with_capacity(8 * 1024) }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
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
                    return out;
                }
                out.push(data.to_string());
            }
        }
        out
    }
}
