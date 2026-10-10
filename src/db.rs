//! SQLite persistence: chats, messages (with their block arrays), uploads and chunks.
//! Schema stays close to upstream Vane's drizzle schema so existing data can be imported.

use crate::blocks::Block;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;

const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE IF NOT EXISTS chats (
      id TEXT PRIMARY KEY,
      title TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL DEFAULT '',
      sources TEXT NOT NULL DEFAULT '[]',
      files TEXT NOT NULL DEFAULT '[]'
    );
    CREATE TABLE IF NOT EXISTS messages (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      message_id TEXT NOT NULL,
      chat_id TEXT NOT NULL,
      role TEXT NOT NULL DEFAULT 'user',
      query TEXT NOT NULL DEFAULT '',
      response TEXT NOT NULL DEFAULT '',
      created_at TEXT NOT NULL,
      response_blocks TEXT NOT NULL DEFAULT '[]',
      files TEXT NOT NULL DEFAULT '[]'
    );
    CREATE INDEX IF NOT EXISTS idx_messages_chat ON messages(chat_id, id);

    CREATE TABLE IF NOT EXISTS settings (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS documents (
      id TEXT PRIMARY KEY,
      chat_id TEXT NOT NULL DEFAULT '',
      name TEXT NOT NULL,
      created_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS chunks (
      id TEXT PRIMARY KEY,
      document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
      idx INTEGER NOT NULL,
      content TEXT NOT NULL,
      embedding BLOB
    );
    CREATE INDEX IF NOT EXISTS idx_chunks_doc ON chunks(document_id);
    "#,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub sources: Vec<String>,
    pub files: Vec<FileRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub blocks: Vec<Block>,
    pub files: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub id: String,
    pub file_id: String,
    pub filename: String,
    pub chunk_index: i64,
    pub content: String,
    pub embedding: Vec<f32>,
}

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    pub path: String,
}

impl Db {
    pub async fn connect(path: &str) -> anyhow::Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let opts = SqliteConnectOptions::from_str(path)
            .with_context(|| format!("invalid sqlite path {path}"))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(10))
            .statement_cache_capacity(64);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect_with(opts)
            .await?;
        for m in MIGRATIONS {
            sqlx::raw_sql(m).execute(&pool).await?;
        }
        Ok(Self { pool, path: path.to_string() })
    }


    pub async fn list_chats(&self, limit: i64) -> anyhow::Result<Vec<Chat>> {
        let rows = sqlx::query(
            "SELECT c.id, c.title, c.created_at, c.updated_at, c.sources, c.files,
                    (SELECT MAX(m.created_at) FROM messages m WHERE m.chat_id = c.id) AS last
             FROM chats c
             ORDER BY COALESCE(last, c.created_at) DESC
             LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| Chat {
                id: r.get("id"),
                title: r.get("title"),
                created_at: r.get("created_at"),
                updated_at: r.get::<Option<String>, _>("last").unwrap_or_else(|| r.get("updated_at")),
                sources: parse_json(r.get("sources")),
                files: parse_json(r.get("files")),
            })
            .collect())
    }

    pub async fn create_chat(&self, id: &str, now: &str) -> anyhow::Result<Chat> {
        sqlx::query("INSERT OR IGNORE INTO chats (id, title, created_at, updated_at) VALUES (?, '', ?, ?)")
            .bind(id)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(Chat {
            id: id.to_string(),
            title: String::new(),
            created_at: now.to_string(),
            updated_at: now.to_string(),
            sources: vec![],
            files: vec![],
        })
    }

    pub async fn get_chat(&self, id: &str) -> anyhow::Result<Option<Chat>> {
        let row = sqlx::query("SELECT id, title, created_at, updated_at, sources, files FROM chats WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| Chat {
            id: r.get("id"),
            title: r.get("title"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
            sources: parse_json(r.get("sources")),
            files: parse_json(r.get("files")),
        }))
    }

    pub async fn delete_chat(&self, id: &str) -> anyhow::Result<bool> {
        let files: Vec<String> = sqlx::query("SELECT id FROM documents WHERE chat_id = ?")
            .bind(id)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| r.get("id"))
            .collect();
        for f in &files {
            sqlx::query("DELETE FROM chunks WHERE document_id = ?").bind(f).execute(&self.pool).await?;
        }
        sqlx::query("DELETE FROM documents WHERE chat_id = ?").bind(id).execute(&self.pool).await?;
        sqlx::query("DELETE FROM messages WHERE chat_id = ?").bind(id).execute(&self.pool).await?;
        let res = sqlx::query("DELETE FROM chats WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn rename_chat(&self, id: &str, title: &str, emoji: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE chats SET title = ? WHERE id = ?")
            .bind(format!("{emoji} {title}").trim().to_string())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_messages(&self, chat_id: &str) -> anyhow::Result<Vec<ChatMessage>> {
        let rows = sqlx::query(
            "SELECT message_id, role, query, response, response_blocks, files, created_at
             FROM messages WHERE chat_id = ? ORDER BY id ASC",
        )
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let role: String = r.get("role");
                let query: String = r.get("query");
                let response: String = r.get("response");
                ChatMessage {
                    id: r.get("message_id"),
                    content: if role == "assistant" { response } else { query },
                    role,
                    blocks: parse_json(r.get("response_blocks")),
                    files: parse_json(r.get("files")),
                    created_at: r.get("created_at"),
                }
            })
            .collect())
    }

    pub async fn save_message(&self, chat_id: &str, msg: &ChatMessage) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO messages (message_id, chat_id, role, query, response, created_at, response_blocks, files)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&msg.id)
        .bind(chat_id)
        .bind(&msg.role)
        .bind(if msg.role == "assistant" { "" } else { &msg.content })
        .bind(if msg.role == "assistant" { &msg.content } else { "" })
        .bind(&msg.created_at)
        .bind(serde_json::to_string(&msg.blocks)?)
        .bind(serde_json::to_string(&msg.files)?)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE chats SET updated_at = ? WHERE id = ?")
            .bind(&msg.created_at)
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_file(&self, chat_id: &str, file_id: &str, name: &str, now: &str) -> anyhow::Result<()> {
        sqlx::query("INSERT OR REPLACE INTO documents (id, chat_id, name, created_at) VALUES (?, ?, ?, ?)")
            .bind(file_id)
            .bind(chat_id)
            .bind(name)
            .bind(now)
            .execute(&self.pool)
            .await?;
        // keep the chat's file list in sync
        let files: Vec<FileRef> = sqlx::query("SELECT id, name FROM documents WHERE chat_id = ?")
            .bind(chat_id)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| FileRef {
                id: r.get("id"),
                name: r.get("name"),
            })
            .collect();
        sqlx::query("UPDATE chats SET files = ? WHERE id = ?")
            .bind(serde_json::to_string(&files)?)
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_chunks(&self, rows: &[ChunkRow]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        for r in rows {
            sqlx::query("INSERT INTO chunks (id, document_id, idx, content, embedding) VALUES (?, ?, ?, ?, ?)")
                .bind(&r.id)
                .bind(&r.file_id)
                .bind(r.chunk_index)
                .bind(&r.content)
                .bind(encode_embedding(&r.embedding))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn chunks_for_files(&self, file_ids: &[String]) -> anyhow::Result<Vec<ChunkRow>> {
        if file_ids.is_empty() {
            return Ok(vec![]);
        }
        // sqlx has no array bind for IN; build the placeholder list
        let placeholders = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT c.id, c.document_id, d.name, c.idx, c.content, c.embedding
             FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.document_id IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql);
        for id in file_ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| ChunkRow {
                id: r.get("id"),
                file_id: r.get("document_id"),
                filename: r.get("name"),
                chunk_index: r.get("idx"),
                content: r.get("content"),
                embedding: decode_embedding(&r.get::<Vec<u8>, _>("embedding")),
            })
            .collect())
    }


}

fn parse_json<T: for<'de> Deserialize<'de> + Default>(raw: String) -> T {
    serde_json::from_str(&raw).unwrap_or_default()
}

fn encode_embedding(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}
