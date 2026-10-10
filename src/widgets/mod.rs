//! Answer widgets: calculator, time, weather, stocks. Ported from upstream `widgets/`.

mod calculator;
pub mod stocks;
pub mod weather;

use crate::blocks::{Block, BlockType};
use crate::config::SettingsHandle;
use calculator::eval;
use chrono::Local;
use chrono_tz::Tz;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Deserialize)]
pub struct WidgetRequest {
    pub name: String,
    #[serde(default)]
    pub input: String,
}

pub struct Registry {
    cfg: SettingsHandle,
    http: Client,
}

impl Registry {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self { http: http.clone(), cfg }
    }

    pub fn available(&self) -> Vec<&'static str> {
        vec!["calculator", "time", "weather", "stocks"]
    }

    pub async fn render(&self, requested: &[WidgetRequest], query: &str, disabled: &[String]) -> Vec<Block> {
        let cfg = self.cfg.get();
        let mut blocks = Vec::new();
        for w in requested.iter().take(4) {
            let name = w.name.to_lowercase();
            if disabled.iter().any(|d| d.eq_ignore_ascii_case(&name)) || !self.available().contains(&name.as_str()) {
                continue;
            }
            let input = w.input.trim().to_string();
            let block = match name.as_str() {
                "calculator" => Self::calculator(&input, query),
                "time" => Self::time(&input),
                "weather" => weather::current(&self.http, &cfg, &input, query).await,
                "stocks" => stocks::quote(&self.http, &input, query).await,
                _ => None,
            };
            if let Some(b) = block {
                blocks.push(b);
            }
        }
        blocks
    }

    fn calculator(input: &str, query: &str) -> Option<Block> {
        let expr = if input.is_empty() { query } else { &input };
        let value = eval(expr).ok()?;
        Some(Block {
            block_type: BlockType::Widget,
            message: format!("{expr} = {value}"),
            data: Some(json!({ "name": "calculator", "expression": expr, "result": value })),
            extra: None,
        })
    }

    fn time(input: &str) -> Option<Block> {
        let now = Local::now();
        let (label, local) = if input.is_empty() {
            ("local".to_string(), now.format("%Y-%m-%d %H:%M:%S %:z").to_string())
        } else {
            match input.parse::<Tz>() {
                Ok(tz) => (input.to_string(), now.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S %:z").to_string()),
                Err(_) => return None,
            }
        };
        Some(Block {
            block_type: BlockType::Widget,
            message: local.clone(),
            data: Some(json!({ "name": "time", "timezone": label, "time": local })),
            extra: None,
        })
    }
}
