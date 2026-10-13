//! Currency conversion for the calculator widget.
//!
//! Rates come from Frankfurter (the ECB's daily reference rates — open data, no key), with
//! open.er-api.com as a fallback for the ~130 currencies the ECB does not publish. One table
//! per base currency, cached for 6 hours; both providers are free and update once a day.

use crate::blocks::{Block, BlockType};
use crate::cache::Cache;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const TTL: u64 = 6 * 3600;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Table {
    rates: Value,
    date: String,
    source: String,
}

/// `100 USD to EUR`, `convert 1,250.50 jpy in gbp`, `how much is 500 EUR in USD?`.
///
/// ISO codes only: `$` and `¥` each belong to a dozen currencies, and the widget model is
/// told in the prompt to write the codes.
pub fn parse(input: &str) -> Option<(f64, String, String)> {
    let s = input
        .trim()
        .trim_end_matches(['?', '!', '.'])
        .to_lowercase()
        .replace('→', " to ")
        .replace("->", " to ")
        .replace('=', " to ");
    let mut split: Option<(usize, usize)> = None;
    for c in [" to ", " in ", " as "] {
        if let Some(i) = s.find(c)
            && split.is_none_or(|(j, _)| i < j)
        {
            split = Some((i, i + c.len()));
        }
    }
    let (start, end) = split?;
    let (left, right) = (&s[..start], s[end..].trim());
    let from = code(left)?;
    let to = code(right)?;
    let amount = amount(left)?;
    Some((amount, from, to))
}

/// A trailing (or leading, for `usd 100`) three-letter ISO code.
fn code(s: &str) -> Option<String> {
    let clean = |w: &str| {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        (w.len() == 3 && w.bytes().all(|b| b.is_ascii_alphabetic())).then(|| w.to_uppercase())
    };
    let mut words = s.split_whitespace();
    words
        .next_back()
        .and_then(clean)
        .or_else(|| words.next().and_then(clean))
}

/// The number in the phrase, with thousands separators removed.
fn amount(s: &str) -> Option<f64> {
    let digits: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    digits
        .replace(',', "")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

fn num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The rate table for one base currency, from whichever provider answers first.
async fn table(http: &Client, cache: &Cache, base: &str) -> Option<Table> {
    let key = format!("fx:{base}");
    if let Some(hit) = cache.get_json::<Table>(&key).await {
        return Some(hit);
    }
    let providers = [
        (
            format!("https://api.frankfurter.dev/v1/latest?base={base}"),
            "date",
            "European Central Bank",
        ),
        (
            format!("https://open.er-api.com/v6/latest/{base}"),
            "time_last_update_utc",
            "ExchangeRate-API",
        ),
    ];
    for (url, date_key, source) in providers {
        let Ok(res) = http.get(url).send().await else {
            continue;
        };
        if !res.status().is_success() {
            continue;
        }
        let Ok(body) = res.json::<Value>().await else {
            continue;
        };
        if let Some(rates) = body["rates"].as_object() {
            let hit = Table {
                rates: Value::Object(rates.clone()),
                date: body[date_key].as_str().unwrap_or_default().to_string(),
                source: source.to_string(),
            };
            cache.set_json(&key, &hit, TTL).await;
            return Some(hit);
        }
    }
    None
}

/// The widget block, or nothing when the phrase is not a conversion or no provider
/// lists that pair — the answer model can still reply from search.
pub async fn block(http: &Client, cache: &Cache, input: &str, query: &str) -> Option<Block> {
    let (value, from, to) = parse(input).or_else(|| parse(query))?;
    let hit = table(http, cache, &from).await?;
    let rate = hit.rates.get(&to)?.as_f64()?;
    let result = (value * rate * 1e6).round() / 1e6;
    Some(Block {
        block_type: BlockType::Widget,
        message: format!("{} {} = {} {}", num(value), from, num(result), to),
        data: Some(json!({
            "name": "currency",
            "amount": value,
            "from": from,
            "to": to,
            "rate": rate,
            "result": result,
            "date": hit.date,
            "source": hit.source,
        })),
        extra: None,
    })
}

/// The converted numbers, so the answer states them instead of inventing an exchange rate.
pub fn facts(blocks: &[Block]) -> String {
    let lines: Vec<String> = blocks
        .iter()
        .filter(|b| {
            b.data
                .as_ref()
                .and_then(|d| d.get("name"))
                .and_then(serde_json::Value::as_str)
                == Some("currency")
        })
        .map(|b| b.message.clone())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "Currency (converted at the daily reference rate shown — state these numbers exactly, in the user's language, and name the rate's date and source; do not cite a search result for them):\n{}",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(input: &str) -> Option<(f64, String, String)> {
        parse(input)
    }

    #[test]
    fn reads_the_ways_a_person_asks_for_a_conversion() {
        assert_eq!(
            pair("100 USD to EUR"),
            Some((100.0, "USD".into(), "EUR".into()))
        );
        assert_eq!(
            pair("convert 1,250.50 jpy in gbp"),
            Some((1250.5, "JPY".into(), "GBP".into()))
        );
        assert_eq!(
            pair("how much is 500 EUR in USD?"),
            Some((500.0, "EUR".into(), "USD".into()))
        );
        assert_eq!(
            pair("HKD 1200 → SGD"),
            Some((1200.0, "HKD".into(), "SGD".into()))
        );
        assert_eq!(
            pair("300 CHF as SEK"),
            Some((300.0, "CHF".into(), "SEK".into()))
        );
    }

    #[test]
    fn leaves_everything_else_to_the_arithmetic_parser() {
        assert_eq!(pair("12 + 7"), None);
        assert_eq!(pair("2 dollars in euros"), None);
        assert_eq!(pair("weather in Paris"), None);
        assert_eq!(pair("what happened in 1969"), None);
        assert_eq!(pair("usd to eur"), None);
        assert_eq!(pair(""), None);
    }

    #[test]
    fn numbers_keep_their_decimals_only_when_they_have_them() {
        assert_eq!(num(100.0), "100");
        assert_eq!(num(1250.5), "1250.5");
        assert_eq!(num(0.89381), "0.89381");
    }

    #[test]
    fn an_unknown_target_currency_is_a_miss_not_a_panic() {
        let hit = Table {
            rates: json!({ "EUR": 0.89 }),
            date: "2026-10-13".into(),
            source: "test".into(),
        };
        assert_eq!(hit.rates.get("RUB").and_then(|v| v.as_f64()), None);
    }

    #[test]
    fn the_answer_is_told_the_converted_number() {
        let money = Block {
            block_type: BlockType::Widget,
            message: "100 USD = 89.381 EUR".into(),
            data: Some(json!({ "name": "currency" })),
            extra: None,
        };
        let text = Block::message("100 USD = 89.381 EUR");
        assert!(facts(&[money.clone()]).contains("100 USD = 89.381 EUR"));
        assert!(facts(&[text]).is_empty(), "only currency blocks count");
    }

    #[tokio::test]
    #[ignore = "hits the rate providers"]
    async fn live_usd_to_eur() {
        let http = Client::new();
        let cache = Cache::new("", false).await;
        let b = block(&http, &cache, "100 USD to EUR", "")
            .await
            .expect("a block");
        let data = b.data.expect("data");
        assert_eq!(data["from"], "USD");
        assert!(data["result"].as_f64().unwrap() > 0.0);
    }
}
