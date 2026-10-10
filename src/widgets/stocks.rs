//! Stock quotes from Stooq (keyless CSV), like upstream Vane's stock widget.

use crate::blocks::{Block, BlockType};
use reqwest::Client;
use serde_json::json;

pub async fn quote(http: &Client, symbol: &str, query: &str) -> Option<Block> {
    let raw = if symbol.is_empty() { query } else { symbol };
    let ticker = raw
        .split_whitespace()
        .last()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_lowercase();
    if ticker.is_empty() || ticker.len() > 8 {
        return None;
    }
    let code = if ticker.contains('.') { ticker.clone() } else { format!("{ticker}.us") };
    let text = http
        .get(format!("https://stooq.com/q/l/?s={code}&f=sd2t2ohlcv&h&e=csv"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    let mut rows = text.lines();
    rows.next()?;
    let cols: Vec<&str> = rows.next()?.split(',').collect();
    if cols.len() < 7 || cols[6] == "N/D" {
        return None;
    }
    let num = |i: usize| cols.get(i).and_then(|v| v.parse::<f64>().ok());
    Some(Block {
        block_type: BlockType::Stocks,
        message: format!("{} {}", cols[0].to_uppercase(), cols[6]),
        data: Some(json!({
            "symbol": cols[0],
            "date": cols[1],
            "time": cols[2],
            "open": num(3),
            "high": num(4),
            "low": num(5),
            "close": num(6),
            "volume": num(7),
        })),
        extra: None,
    })
}
