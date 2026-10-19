//! SQLite persistence: chats, messages (with their block arrays), uploads and chunks.
//! Schema stays close to the original project's drizzle schema so existing data can be imported.

use crate::blocks::Block;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
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
    r#"
    CREATE TABLE IF NOT EXISTS jobs (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      kind TEXT NOT NULL DEFAULT 'command',
      cron TEXT NOT NULL DEFAULT '',
      timezone TEXT NOT NULL DEFAULT '',
      enabled INTEGER NOT NULL DEFAULT 1,
      image TEXT NOT NULL DEFAULT 'alpine',
      command TEXT NOT NULL DEFAULT '',
      prompt TEXT NOT NULL DEFAULT '',
      memory TEXT NOT NULL DEFAULT '256M',
      max_duration_secs INTEGER NOT NULL DEFAULT 300,
      notify_url TEXT NOT NULL DEFAULT '',
      created_at TEXT NOT NULL,
      next_run_at TEXT NOT NULL DEFAULT '',
      last_run_at TEXT NOT NULL DEFAULT '',
      last_status TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE IF NOT EXISTS job_runs (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
      started_at TEXT NOT NULL,
      finished_at TEXT NOT NULL DEFAULT '',
      status TEXT NOT NULL DEFAULT 'running',
      exit_code INTEGER,
      output TEXT NOT NULL DEFAULT '',
      detail TEXT NOT NULL DEFAULT '',
      chat_id TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX IF NOT EXISTS idx_job_runs ON job_runs(job_id, id DESC);
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
    /// Phase of the latest answer: search, read, write, done, stopped, error. Empty when the thread has no answers yet.
    #[serde(default)]
    pub status: String,
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
    /// search | read | write while an answer is in progress, then done, stopped, or error.
    #[serde(default = "done_status")]
    pub status: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<i64>,
}

fn done_status() -> String {
    "done".into()
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

/// A scheduled job the Automator owns. `cron` is stored normalized (seconds first);
/// `timezone` is an IANA name, empty meaning the server's own local zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// "command": one shell line inside a throwaway microVM. "prompt": a question run
    /// through the answer pipeline, so each run lands as a thread in the sidebar.
    #[serde(default = "command_kind")]
    pub kind: String,
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default = "default_image")]
    pub image: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default = "default_memory")]
    pub memory: String,
    #[serde(default = "default_duration")]
    pub max_duration_secs: i64,
    /// Webhook POSTed after every run — an ntfy topic, a Slack-compatible hook, your own.
    #[serde(default)]
    pub notify_url: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub next_run_at: String,
    #[serde(default)]
    pub last_run_at: String,
    #[serde(default)]
    pub last_status: String,
}

fn command_kind() -> String {
    "command".into()
}
fn on() -> bool {
    true
}
fn default_image() -> String {
    "alpine".into()
}
fn default_memory() -> String {
    "256M".into()
}
fn default_duration() -> i64 {
    300
}

/// One execution of a job. `chat_id` is set for prompt jobs, output is capped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRun {
    pub id: i64,
    pub job_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub output: String,
    pub detail: String,
    pub chat_id: String,
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
        // Columns added after the first release. CREATE TABLE IF NOT EXISTS will not
        // alter a database that already exists, so each one is added on its own.
        ensure_column(&pool, "messages", "status", "TEXT NOT NULL DEFAULT 'done'").await?;
        ensure_column(&pool, "messages", "error", "TEXT NOT NULL DEFAULT ''").await?;
        ensure_column(&pool, "messages", "model", "TEXT NOT NULL DEFAULT ''").await?;
        ensure_column(&pool, "messages", "elapsed_ms", "INTEGER").await?;
        ensure_column(&pool, "documents", "kind", "TEXT NOT NULL DEFAULT 'text'").await?;
        ensure_column(&pool, "documents", "mime", "TEXT NOT NULL DEFAULT ''").await?;
        ensure_column(&pool, "documents", "data", "BLOB").await?;
        // A turn left in progress belonged to a process that is no longer running.
        sqlx::query(
            "UPDATE messages SET status = 'stopped'
             WHERE role = 'assistant' AND status IN ('search', 'read', 'write')",
        )
        .execute(&pool)
        .await?;
        // Same for a job run whose microVM or answer died with the previous process.
        sqlx::query("UPDATE job_runs SET status = 'stopped' WHERE status = 'running'")
            .execute(&pool)
            .await?;
        Ok(Self {
            pool,
            path: path.to_string(),
        })
    }

    pub async fn list_chats(&self, limit: i64) -> anyhow::Result<Vec<Chat>> {
        let rows = sqlx::query(
            "SELECT c.id, c.title, c.created_at, c.updated_at, c.sources, c.files,
                    (SELECT MAX(m.created_at) FROM messages m WHERE m.chat_id = c.id) AS last,
                    (SELECT m.status FROM messages m
                      WHERE m.chat_id = c.id AND m.role = 'assistant'
                      ORDER BY m.id DESC LIMIT 1) AS status
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
                updated_at: r
                    .get::<Option<String>, _>("last")
                    .unwrap_or_else(|| r.get("updated_at")),
                sources: parse_json(r.get("sources")),
                files: parse_json(r.get("files")),
                status: r.get::<Option<String>, _>("status").unwrap_or_default(),
            })
            .collect())
    }

    pub async fn create_chat(&self, id: &str, now: &str) -> anyhow::Result<Chat> {
        sqlx::query(
            "INSERT OR IGNORE INTO chats (id, title, created_at, updated_at) VALUES (?, '', ?, ?)",
        )
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
            status: String::new(),
        })
    }

    pub async fn get_chat(&self, id: &str) -> anyhow::Result<Option<Chat>> {
        let row = sqlx::query(
            "SELECT id, title, created_at, updated_at, sources, files,
                    (SELECT m.status FROM messages m
                      WHERE m.chat_id = chats.id AND m.role = 'assistant'
                      ORDER BY m.id DESC LIMIT 1) AS status
             FROM chats WHERE id = ?",
        )
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
            status: r.get::<Option<String>, _>("status").unwrap_or_default(),
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
            sqlx::query("DELETE FROM chunks WHERE document_id = ?")
                .bind(f)
                .execute(&self.pool)
                .await?;
        }
        sqlx::query("DELETE FROM documents WHERE chat_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM messages WHERE chat_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        let res = sqlx::query("DELETE FROM chats WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
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
            "SELECT message_id, role, query, response, response_blocks, files, created_at,
                    status, error, model, elapsed_ms
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
                    status: r
                        .get::<Option<String>, _>("status")
                        .unwrap_or_else(done_status),
                    error: r.get::<Option<String>, _>("error").unwrap_or_default(),
                    model: r.get::<Option<String>, _>("model").unwrap_or_default(),
                    elapsed_ms: r.get("elapsed_ms"),
                }
            })
            .collect())
    }

    pub async fn save_message(&self, chat_id: &str, msg: &ChatMessage) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO messages
                (message_id, chat_id, role, query, response, created_at, response_blocks, files, status, error, model, elapsed_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&msg.id)
        .bind(chat_id)
        .bind(&msg.role)
        .bind(if msg.role == "assistant" { "" } else { &msg.content })
        .bind(if msg.role == "assistant" { &msg.content } else { "" })
        .bind(&msg.created_at)
        .bind(serde_json::to_string(&msg.blocks)?)
        .bind(serde_json::to_string(&msg.files)?)
        .bind(if msg.status.is_empty() { "done".to_string() } else { msg.status.clone() })
        .bind(&msg.error)
        .bind(&msg.model)
        .bind(msg.elapsed_ms)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE chats SET updated_at = ? WHERE id = ?")
            .bind(&msg.created_at)
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Store the question immediately and open an assistant row in the `search` phase.
    /// Returns `(user_message_id, assistant_message_id)`.
    pub async fn open_turn(
        &self,
        chat_id: &str,
        query: &str,
        files: &[String],
        model: &str,
        now: &str,
    ) -> anyhow::Result<(String, String)> {
        let user_id = uuid::Uuid::new_v4().to_string();
        let assistant_id = uuid::Uuid::new_v4().to_string();
        let title = provisional_title(query);
        let files_json = serde_json::to_string(files)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE chats SET title = ?, updated_at = ? WHERE id = ? AND title = ''")
            .bind(&title)
            .bind(now)
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO messages
                (message_id, chat_id, role, query, response, created_at, response_blocks, files, status, error, model)
             VALUES (?, ?, 'user', ?, '', ?, '[]', ?, 'done', '', '')",
        )
        .bind(&user_id)
        .bind(chat_id)
        .bind(query)
        .bind(now)
        .bind(&files_json)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO messages
                (message_id, chat_id, role, query, response, created_at, response_blocks, files, status, error, model)
             VALUES (?, ?, 'assistant', '', '', ?, '[]', '[]', 'search', '', ?)",
        )
        .bind(&assistant_id)
        .bind(chat_id)
        .bind(now)
        .bind(model)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((user_id, assistant_id))
    }

    /// Update the in-progress answer. A later `write` cannot revive a turn already marked stopped,
    /// and `done` / `error` cannot overwrite `stopped`.
    pub async fn write_turn(
        &self,
        chat_id: &str,
        assistant_id: &str,
        answer: &str,
        blocks: &[Block],
        phase: &str,
        error: &str,
        model: &str,
        elapsed_ms: i64,
        now: &str,
    ) -> anyhow::Result<()> {
        let blocks_json = serde_json::to_string(blocks)?;
        let res = sqlx::query(
            "UPDATE messages SET
                response = ?,
                response_blocks = ?,
                status = ?,
                error = ?,
                model = ?,
                elapsed_ms = ?
             WHERE message_id = ? AND chat_id = ? AND role = 'assistant'
               AND (
                 (? IN ('search', 'read', 'write') AND status IN ('search', 'read', 'write'))
                 OR ? = 'stopped'
                 OR (? IN ('done', 'error') AND status != 'stopped')
               )",
        )
        .bind(answer)
        .bind(&blocks_json)
        .bind(phase)
        .bind(error)
        .bind(model)
        .bind(elapsed_ms)
        .bind(assistant_id)
        .bind(chat_id)
        .bind(phase)
        .bind(phase)
        .bind(phase)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() > 0 {
            sqlx::query("UPDATE chats SET updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(chat_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Mark every in-progress answer on this thread stopped. The Stop button calls this
    /// so a refresh already shows the stopped phase. A refresh by itself does not.
    pub async fn mark_stopped(&self, chat_id: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE messages SET status = 'stopped'
             WHERE chat_id = ? AND role = 'assistant' AND status IN ('search', 'read', 'write')",
        )
        .bind(chat_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_file(
        &self,
        chat_id: &str,
        file_id: &str,
        name: &str,
        now: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO documents (id, chat_id, name, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(file_id)
        .bind(chat_id)
        .bind(name)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.sync_chat_files(chat_id).await
    }

    /// Store an image as itself. It is not chunked or embedded; the answer model sees the bytes.
    pub async fn save_image(
        &self,
        chat_id: &str,
        file_id: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
        now: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO documents (id, chat_id, name, created_at, kind, mime, data)
             VALUES (?, ?, ?, ?, 'image', ?, ?)",
        )
        .bind(file_id)
        .bind(chat_id)
        .bind(name)
        .bind(now)
        .bind(mime)
        .bind(bytes)
        .execute(&self.pool)
        .await?;
        self.sync_chat_files(chat_id).await
    }

    pub async fn images_for(&self, file_ids: &[String]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        if file_ids.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = file_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT mime, data FROM documents
             WHERE kind = 'image' AND data IS NOT NULL AND id IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql);
        for id in file_ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| (r.get("mime"), r.get("data")))
            .collect())
    }

    async fn sync_chat_files(&self, chat_id: &str) -> anyhow::Result<()> {
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

    // ------------------------------------------------------------- automator

    pub async fn list_jobs(&self) -> anyhow::Result<Vec<Job>> {
        let rows = sqlx::query(JOB_SELECT).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(job_from_row).collect())
    }

    pub async fn get_job(&self, id: &str) -> anyhow::Result<Option<Job>> {
        let row = sqlx::query(&format!("{JOB_SELECT} WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(job_from_row))
    }

    /// Jobs whose computed next run has come due. UTC stamps sort as text, so `<=` works.
    pub async fn due_jobs(&self, now: &str) -> anyhow::Result<Vec<Job>> {
        let rows = sqlx::query(&format!(
            "{JOB_SELECT} WHERE enabled = 1 AND next_run_at != '' AND next_run_at <= ? ORDER BY next_run_at"
        ))
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(job_from_row).collect())
    }

    pub async fn save_job(&self, j: &Job) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO jobs (id, name, kind, cron, timezone, enabled, image, command, prompt,
                                 memory, max_duration_secs, notify_url, created_at, next_run_at, last_run_at, last_status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, kind = excluded.kind,
                 cron = excluded.cron, timezone = excluded.timezone, enabled = excluded.enabled,
                 image = excluded.image, command = excluded.command, prompt = excluded.prompt,
                 memory = excluded.memory, max_duration_secs = excluded.max_duration_secs,
                 notify_url = excluded.notify_url, next_run_at = excluded.next_run_at,
                 last_run_at = excluded.last_run_at, last_status = excluded.last_status"#,
        )
        .bind(&j.id)
        .bind(&j.name)
        .bind(&j.kind)
        .bind(&j.cron)
        .bind(&j.timezone)
        .bind(j.enabled)
        .bind(&j.image)
        .bind(&j.command)
        .bind(&j.prompt)
        .bind(&j.memory)
        .bind(j.max_duration_secs)
        .bind(&j.notify_url)
        .bind(&j.created_at)
        .bind(&j.next_run_at)
        .bind(&j.last_run_at)
        .bind(&j.last_status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_job(&self, id: &str) -> anyhow::Result<bool> {
        let res = sqlx::query("DELETE FROM jobs WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Record the next due time (and, for a firing job, the previous one + its state).
    pub async fn bump_job(
        &self,
        id: &str,
        next_run_at: &str,
        last_run_at: &str,
        status: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE jobs SET next_run_at = ?, last_run_at = ?, last_status = ? WHERE id = ?",
        )
        .bind(next_run_at)
        .bind(last_run_at)
        .bind(status)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn start_job_run(&self, job_id: &str, started_at: &str) -> anyhow::Result<i64> {
        let res = sqlx::query("INSERT INTO job_runs (job_id, started_at) VALUES (?, ?)")
            .bind(job_id)
            .bind(started_at)
            .execute(&self.pool)
            .await?;
        Ok(res.last_insert_rowid())
    }

    /// Close a run row, mirror its state onto the job, and keep only the newest `KEEP_RUNS`.
    pub async fn finish_job_run(&self, run: &JobRun) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE job_runs SET finished_at = ?, status = ?, exit_code = ?, output = ?, detail = ?, chat_id = ?
             WHERE id = ?",
        )
        .bind(&run.finished_at)
        .bind(&run.status)
        .bind(run.exit_code)
        .bind(&run.output)
        .bind(&run.detail)
        .bind(&run.chat_id)
        .bind(run.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE jobs SET last_status = ? WHERE id = ?")
            .bind(&run.status)
            .bind(&run.job_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM job_runs WHERE job_id = ? AND id NOT IN
                (SELECT id FROM job_runs WHERE job_id = ? ORDER BY id DESC LIMIT ?)",
        )
        .bind(&run.job_id)
        .bind(&run.job_id)
        .bind(JOB_RUN_HISTORY)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_job_runs(&self, job_id: &str, limit: i64) -> anyhow::Result<Vec<JobRun>> {
        let rows = sqlx::query(
            "SELECT id, job_id, started_at, finished_at, status, exit_code, output, detail, chat_id
             FROM job_runs WHERE job_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(job_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| JobRun {
                id: r.get("id"),
                job_id: r.get("job_id"),
                started_at: r.get("started_at"),
                finished_at: r.get("finished_at"),
                status: r.get("status"),
                exit_code: r.get("exit_code"),
                output: r.get("output"),
                detail: r.get("detail"),
                chat_id: r.get("chat_id"),
            })
            .collect())
    }
}

const JOB_SELECT: &str = "SELECT id, name, kind, cron, timezone, enabled, image, command, prompt,
                          memory, max_duration_secs, notify_url, created_at, next_run_at,
                          last_run_at, last_status FROM jobs";

/// How much run history to keep per job — enough to see a weekly job fail twice.
const JOB_RUN_HISTORY: i64 = 20;

fn job_from_row(r: &sqlx::sqlite::SqliteRow) -> Job {
    Job {
        id: r.get("id"),
        name: r.get("name"),
        kind: r.get("kind"),
        cron: r.get("cron"),
        timezone: r.get("timezone"),
        enabled: r.get::<i64, _>("enabled") != 0,
        image: r.get("image"),
        command: r.get("command"),
        prompt: r.get("prompt"),
        memory: r.get("memory"),
        max_duration_secs: r.get("max_duration_secs"),
        notify_url: r.get("notify_url"),
        created_at: r.get("created_at"),
        next_run_at: r.get("next_run_at"),
        last_run_at: r.get("last_run_at"),
        last_status: r.get("last_status"),
    }
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    def: &str,
) -> anyhow::Result<()> {
    let present: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"
    ))
    .fetch_one(pool)
    .await?;
    if present == 0 {
        sqlx::raw_sql(&format!("ALTER TABLE {table} ADD COLUMN {column} {def}"))
            .execute(pool)
            .await?;
    }
    Ok(())
}

fn provisional_title(query: &str) -> String {
    let flat = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for ch in flat.chars() {
        if out.chars().count() >= 80 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> Db {
        let path = std::env::temp_dir().join(format!("libra-turn-{}.sqlite", uuid::Uuid::new_v4()));
        Db::connect(path.to_str().unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn turn_state_is_stored_and_stop_sticks() {
        let db = fresh().await;
        db.create_chat("c1", "t0").await.unwrap();
        let (_user, asst) = db
            .open_turn(
                "c1",
                "why is the sky blue",
                &["file-1".into()],
                "mock",
                "t1",
            )
            .await
            .unwrap();

        let chat = db.get_chat("c1").await.unwrap().unwrap();
        assert_eq!(chat.title, "why is the sky blue");
        assert_eq!(chat.status, "search");

        let msgs = db.get_messages("c1").await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "why is the sky blue");
        assert_eq!(msgs[0].files, vec!["file-1".to_string()]);
        assert_eq!(msgs[1].id, asst);
        assert_eq!(msgs[1].status, "search");

        db.write_turn("c1", &asst, "Because", &[], "write", "", "mock", 10, "t2")
            .await
            .unwrap();
        let msgs = db.get_messages("c1").await.unwrap();
        assert_eq!(msgs[1].content, "Because");
        assert_eq!(msgs[1].status, "write");
        assert_eq!(msgs[1].elapsed_ms, Some(10));

        db.write_turn("c1", &asst, "Because", &[], "done", "", "mock", 40, "t3")
            .await
            .unwrap();
        assert_eq!(db.get_messages("c1").await.unwrap()[1].status, "done");

        // A second turn can be stopped, and a late progress write must not revive it.
        let (_user, asst) = db
            .open_turn("c1", "and at sunset", &[], "mock", "t4")
            .await
            .unwrap();
        db.write_turn("c1", &asst, "Rayleigh", &[], "write", "", "mock", 5, "t5")
            .await
            .unwrap();
        db.write_turn("c1", &asst, "Rayleigh", &[], "stopped", "", "mock", 8, "t6")
            .await
            .unwrap();
        db.write_turn(
            "c1",
            &asst,
            "Rayleigh scattering",
            &[],
            "write",
            "",
            "mock",
            12,
            "t7",
        )
        .await
        .unwrap();
        db.write_turn(
            "c1",
            &asst,
            "full answer",
            &[],
            "done",
            "",
            "mock",
            20,
            "t8",
        )
        .await
        .unwrap();
        let msgs = db.get_messages("c1").await.unwrap();
        let last = msgs.last().unwrap();
        assert_eq!(last.status, "stopped");
        assert_eq!(last.content, "Rayleigh");
        assert_eq!(db.get_chat("c1").await.unwrap().unwrap().status, "stopped");
        // The first question's title is kept.
        assert_eq!(
            db.get_chat("c1").await.unwrap().unwrap().title,
            "why is the sky blue"
        );
    }

    #[tokio::test]
    async fn restart_marks_open_turns_stopped() {
        let path = std::env::temp_dir().join(format!("libra-turn-{}.sqlite", uuid::Uuid::new_v4()));
        let path = path.to_str().unwrap().to_string();
        {
            let db = Db::connect(&path).await.unwrap();
            db.create_chat("c1", "t0").await.unwrap();
            db.open_turn("c1", "hello", &[], "mock", "t1")
                .await
                .unwrap();
            assert_eq!(db.get_messages("c1").await.unwrap()[1].status, "search");
        }
        let db = Db::connect(&path).await.unwrap();
        assert_eq!(db.get_messages("c1").await.unwrap()[1].status, "stopped");
    }
}
