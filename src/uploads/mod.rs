//! Document uploads: text extraction, chunking, embeddings, cosine retrieval.
//! Replaces upstream's LanceDB vector store with SQLite + in-process cosine, which is
//! plenty for the per-chat chunk counts a self-hosted search tool sees.

use crate::cache::Cache;
use crate::config::SettingsHandle;
use crate::db::Db;
use crate::error::LibraError;
use crate::llm;
use reqwest::Client;
use std::sync::Arc;

const CHUNK_SIZE: usize = 1200;
const CHUNK_OVERLAP: usize = 200;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub filename: String,
    pub content: String,
    pub embedding: Vec<f32>,
}

pub struct UploadStore {
    cfg: SettingsHandle,
    db: Arc<Db>,
    #[allow(dead_code)]
    cache: Cache,
    http: Client,
}

impl UploadStore {
    pub fn new(http: &Client, cfg: SettingsHandle, db: Arc<Db>, cache: Cache) -> Self {
        Self {
            cfg,
            db,
            cache,
            http: http.clone(),
        }
    }

    /// Extract, chunk and embed a file; returns the number of chunks stored.
    pub async fn ingest(
        &self,
        chat_id: &str,
        file_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<usize, LibraError> {
        let cfg = self.cfg.get();
        // Images are stored whole. Running them through the text embedder treats the
        // bytes as a document and the request sits there until the model gives up.
        if let Some(mime) = image_mime(bytes) {
            self.db
                .save_image(chat_id, file_id, filename, mime, bytes, &cfg.now())
                .await?;
            return Ok(0);
        }
        if !is_document(filename) && looks_binary(bytes) {
            return Err(LibraError::BadRequest(format!(
                "{filename} has no readable text. Attach a text file, PDF, Word document, or an image."
            )));
        }
        let text = extract_text(filename, bytes);
        let chunks = split_text(&text);
        if chunks.is_empty() || !readable(&text) {
            return Err(LibraError::BadRequest(format!(
                "no readable text found in {filename} (supported: txt, md, csv, pdf, docx, and images)"
            )));
        }
        let llm_client = llm::resolve_embed(&cfg, &self.http, &cfg.default_embedding_model)?;
        let embeddings = llm_client
            .embed(&chunks)
            .await
            .map_err(|e| LibraError::Provider {
                provider: cfg.default_embedding_model.clone(),
                message: format!("embedding failed: {e}"),
            })?;
        let mut rows = Vec::with_capacity(chunks.len());
        for (i, content) in chunks.iter().enumerate() {
            rows.push(crate::db::ChunkRow {
                id: uuid::Uuid::new_v4().to_string(),
                file_id: file_id.to_string(),
                chunk_index: i as i64,
                content: content.clone(),
                filename: filename.to_string(),
                embedding: embeddings.get(i).cloned().unwrap_or_default(),
            });
        }
        self.db
            .save_file(chat_id, file_id, filename, &cfg.now())
            .await?;
        self.db.save_chunks(&rows).await?;
        Ok(rows.len())
    }

    /// Image bytes for the selected attachments, in storage order. At most four.
    pub async fn images_for(
        &self,
        file_ids: &[String],
    ) -> Result<Vec<(String, Vec<u8>)>, LibraError> {
        let mut images = self.db.images_for(file_ids).await?;
        images.truncate(4);
        Ok(images)
    }

    /// Top-k chunks across the given files, ranked by cosine similarity to the query.
    pub async fn retrieve(
        &self,
        chat_id: &str,
        file_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Chunk>, LibraError> {
        let cfg = self.cfg.get();
        let rows = self.db.chunks_for_files(file_ids).await?;
        if rows.is_empty() {
            return Ok(vec![]);
        }
        let llm_client = llm::resolve_embed(&cfg, &self.http, &cfg.default_embedding_model)?;
        let qemb = llm_client
            .embed(&[query.to_string()])
            .await
            .map_err(|e| LibraError::Provider {
                provider: cfg.default_embedding_model.clone(),
                message: format!("embedding failed: {e}"),
            })?
            .into_iter()
            .next()
            .unwrap_or_default();

        let mut scored: Vec<(f32, Chunk)> = rows
            .into_iter()
            .map(|r| {
                let chunk = Chunk {
                    filename: r.filename,
                    content: r.content,
                    embedding: r.embedding,
                };
                let score = if qemb.is_empty() || chunk.embedding.is_empty() {
                    0.0
                } else {
                    cosine(&qemb, &chunk.embedding)
                };
                (score, chunk)
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let _ = chat_id;
        Ok(scored
            .into_iter()
            .take(limit.max(1))
            .map(|(_, c)| c)
            .collect())
    }
}

/// png, jpeg, gif, webp, or bmp, from the file header.
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, 0x50, 0x4E, 0x47, ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'B', b'M', ..] => Some("image/bmp"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

fn is_document(filename: &str) -> bool {
    matches!(
        filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_lowercase()
            .as_str(),
        "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "log"
            | "html"
            | "htm"
            | "xml"
            | "rst"
            | "yaml"
            | "yml"
            | "pdf"
            | "docx"
    )
}

fn looks_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8000)];
    if sample.is_empty() {
        return true;
    }
    let bad = sample.iter().filter(|b| **b == 0 || **b < 9).count();
    bad * 20 > sample.len()
}

fn readable(text: &str) -> bool {
    let sample: Vec<char> = text.trim().chars().take(4000).collect();
    if sample.is_empty() {
        return false;
    }
    let bad = sample
        .iter()
        .filter(|c| {
            **c == '\u{FFFD}' || (c.is_control() && **c != '\n' && **c != '\r' && **c != '\t')
        })
        .count();
    bad * 5 < sample.len()
}

pub fn extract_text(filename: &str, bytes: &[u8]) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "pdf" => pdf_extract::extract_text_from_mem(bytes).unwrap_or_else(|e| {
            tracing::warn!("pdf parse failed for {filename}: {e}");
            String::new()
        }),
        "docx" => docx_text(bytes).unwrap_or_default(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn docx_text(bytes: &[u8]) -> Result<String, std::io::Error> {
    use std::io::{Cursor, Read};
    let mut cursor = Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(&mut cursor)?;
    let mut xml = String::new();
    if let Ok(mut f) = zip.by_name("word/document.xml") {
        f.read_to_string(&mut xml)?;
    }
    // paragraph breaks, then strip tags
    let xml = xml.replace("</w:p>", "\n");
    let mut out = String::with_capacity(xml.len());
    let mut in_tag = false;
    for c in xml.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    Ok(out)
}

/// Character windows with overlap — same shape as upstream's `splitText`.
pub fn split_text(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + CHUNK_SIZE).min(chars.len());
        let piece: String = chars[i..end].iter().collect();
        if !piece.trim().is_empty() {
            out.push(piece);
        }
        if end == chars.len() {
            break;
        }
        i = end - CHUNK_OVERLAP;
    }
    out
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn chunks_overlap_and_cover() {
        let text = "a".repeat(3000);
        let chunks = split_text(&text);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chars().count(), 1200);
        assert_eq!(
            chunks.iter().map(|c| c.chars().count()).sum::<usize>(),
            3400
        ); // overlap included
    }

    #[test]
    fn cosine_detects_similarity() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn image_headers_are_not_treated_as_text() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        assert_eq!(image_mime(&png), Some("image/png"));
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert!(looks_binary(&png));
        assert!(!looks_binary(b"hello, a plain note\n"));
        assert!(readable("hello, a plain note"));
        assert!(!readable("\u{FFFD}\u{FFFD}\u{FFFD}\u{0000}"));
    }

    #[test]
    fn docx_text_extraction() {
        // minimal docx: one paragraph
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        w.start_file("word/document.xml", opts).unwrap();
        use std::write;
        write!(
            w,
            "<w:document><w:p><w:t>Hello</w:t></w:p><w:p><w:t>World</w:t></w:p></w:document>"
        )
        .unwrap();
        let bytes = w.finish().unwrap().into_inner();
        assert_eq!(docx_text(&bytes).unwrap(), "Hello\nWorld\n");
    }
}
