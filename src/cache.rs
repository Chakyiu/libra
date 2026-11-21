use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

/// Redis caches exactly three things: web search results, Discover articles and
/// currency-rate tables. Embeddings are recomputed for every question and there
/// is no single-flight lock. It is *optional* — every call degrades to a miss
/// when Redis is down so a laptop `cargo run` without Redis still works.
#[derive(Clone)]
pub struct Cache {
    inner: Option<ConnectionManager>,
}

/// `redis://:secret@host:6379/0` → `host:6379`. Log-safe form of a connection
/// URL: scheme, credentials, path and query are dropped.
pub fn redacted_host(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host_port = match rest.rsplit_once('@') {
        Some((_, hp)) => hp,
        None => rest,
    };
    host_port.split('/').next().unwrap_or("").to_string()
}

pub fn hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(32);
    for b in &out[..16] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

impl Cache {
    /// Connect once with a hard deadline: a missing Redis must degrade caching,
    /// never stall startup.
    pub async fn new(url: &str, enabled: bool) -> Self {
        if !enabled {
            tracing::info!("redis disabled by config");
            return Cache { inner: None };
        }
        let client = match redis::Client::open(url) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "bad REDIS_URL, caching disabled");
                return Cache { inner: None };
            }
        };
        let cm_config = redis::aio::ConnectionManagerConfig::new()
            .set_number_of_retries(1)
            .set_min_delay(std::time::Duration::from_millis(50))
            .set_connection_timeout(Some(std::time::Duration::from_secs(2)))
            .set_response_timeout(Some(std::time::Duration::from_secs(3)));
        let mut cm = match client.get_connection_manager_lazy(cm_config) {
            Ok(cm) => cm,
            Err(e) => {
                tracing::warn!(error = %e, "redis unreachable, caching disabled");
                return Cache { inner: None };
            }
        };
        let ping: redis::RedisResult<String> =
            redis::cmd("PING").arg("libra").query_async(&mut cm).await;
        match ping {
            Ok(_) => {
                // Never log the URL: `redis://:password@host:6379` carries a credential.
                tracing::info!(enabled = true, host = %redacted_host(url), "redis connected");
                Cache { inner: Some(cm) }
            }
            Err(e) => {
                tracing::warn!(error = %e, "redis ping failed, caching disabled");
                Cache { inner: None }
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub async fn get_raw(&self, key: &str) -> Option<Vec<u8>> {
        let mut c = self.inner.clone()?;
        c.get::<_, Option<Vec<u8>>>(key).await.ok().flatten()
    }

    pub async fn set_raw(&self, key: &str, value: &[u8], ttl: u64) {
        let Some(mut c) = self.inner.clone() else {
            return;
        };
        let _: redis::RedisResult<()> = redis::cmd("SET")
            .arg(key)
            .arg(value)
            .arg("EX")
            .arg(ttl)
            .query_async(&mut c)
            .await;
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let raw = self.get_raw(key).await?;
        serde_json::from_slice(&raw).ok()
    }

    pub async fn set_json<T: Serialize>(&self, key: &str, value: &T, ttl: u64) {
        if let Ok(raw) = serde_json::to_vec(value) {
            self.set_raw(key, &raw, ttl).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::redacted_host;

    #[test]
    fn log_safe_host_drops_credentials() {
        assert_eq!(
            redacted_host("redis://:hunter2@cache.local:6379"),
            "cache.local:6379"
        );
        assert_eq!(
            redacted_host("rediss://user:p@ss@host.example:6380/0"),
            "host.example:6380"
        );
        assert_eq!(redacted_host("redis://127.0.0.1:6379/1"), "127.0.0.1:6379");
        assert_eq!(redacted_host(""), "");
    }
}
