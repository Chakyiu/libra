mod api;
mod blocks;
mod cache;
mod config;
mod db;
mod discover;
mod error;
mod fetch;
mod llm;
mod media;
mod pipeline;
mod prompts;
mod search;
mod settings;
mod state;
mod suggestions;
mod uploads;
mod widgets;

use crate::config::Config;
use crate::db::Db;
use crate::state::App;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,libra=info,sqlx=warn".into()),
        )
        .init();

    dotenvy::dotenv().ok();
    let cfg = Config::load()?;
    tracing::info!(data_dir = %cfg.data_dir.display(), db = %cfg.db_path.display(), "loaded config");

    let db = Arc::new(Db::connect(&cfg.db_path.to_string_lossy()).await?);
    let cache = crate::cache::Cache::new(&cfg.redis_url, cfg.redis_enabled).await;
    tracing::info!(redis = cache.enabled(), url = %cfg.redis_url, "redis state");

    let settings = crate::config::SettingsHandle::new(cfg.clone());
    let app = Arc::new(App::new(settings, db, cache)?);

    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr.as_str()).await?;
    tracing::info!("Libra listening on http://{}:{}", cfg.host, cfg.port);
    axum::serve(listener, api::router(app))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
