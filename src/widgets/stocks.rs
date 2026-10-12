//! US stock and ETF quotes. Nasdaq supplies the price, the intraday chart and
//! the daily history; the stats row is filled in from the same quote and from
//! StockAnalysis.

use crate::blocks::{Block, BlockType};
use chrono::{DateTime, Datelike, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::America::New_York;
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};

const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[derive(Debug)]
pub enum ChartError {
    /// The symbol or the range is not one this feed can draw.
    Rejected,
    /// The feed had nothing to plot.
    Unavailable,
}

#[derive(Clone, Serialize)]
struct Point {
    t: i64,
    p: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    v: Option<f64>,
}

#[derive(Serialize)]
struct Extended {
    label: String,
    price: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    change: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    change_percent: Option<f64>,
    #[serde(skip_serializing_if = "str::is_empty")]
    as_of: String,
}

#[derive(Serialize)]
struct Quote {
    symbol: String,
    name: String,
    exchange: String,
    currency: &'static str,
    timezone: &'static str,
    country: &'static str,
    price: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    change: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    change_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_close: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    open: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    low: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    market_cap: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trailing_pe: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forward_pe: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    eps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    week_low: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    week_high: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dividend_yield: Option<f64>,
    market_state: String,
    session_label: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    as_of: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    extended: Option<Extended>,
    range: &'static str,
    granularity: &'static str,
    logo: String,
    points: Vec<Point>,
}

#[derive(Serialize)]
struct Series {
    symbol: String,
    range: String,
    granularity: String,
    points: Vec<Point>,
}

struct Facts {
    trailing_pe: Option<f64>,
    forward_pe: Option<f64>,
    eps: Option<f64>,
    market_cap: Option<f64>,
}

pub async fn quote(http: &Client, symbol: &str, query: &str) -> Option<Block> {
    let hit = resolve(http, symbol, query).await?;
    let quote = assemble(http, &hit).await?;
    let message = match (quote.change, quote.change_percent) {
        (Some(change), Some(pct)) => {
            format!(
                "{} {:.2} ({change:+.2}, {pct:+.2}%)",
                quote.symbol, quote.price
            )
        }
        _ => format!("{} {:.2}", quote.symbol, quote.price),
    };
    let data = serde_json::to_value(&quote).ok()?;
    Some(Block {
        block_type: BlockType::Stocks,
        message,
        data: Some(data),
        extra: None,
    })
}

/// A chart for a range chip. `1d` is the intraday session; the rest are daily closes.
pub async fn chart(http: &Client, symbol: &str, range: &str) -> Result<Value, ChartError> {
    let range = canonical_range(range).ok_or(ChartError::Rejected)?;
    let sym = normalize_symbol(symbol);
    if !is_ticker(&sym) {
        return Err(ChartError::Rejected);
    }
    let points = load_points(http, &sym, range).await;
    if points.is_empty() {
        return Err(ChartError::Unavailable);
    }
    let granularity = if range == "1d" { "minute" } else { "day" };
    Ok(serde_json::to_value(Series {
        symbol: sym,
        range: range.to_string(),
        granularity: granularity.to_string(),
        points,
    })
    .unwrap_or_else(|_| json!({})))
}

struct Hit {
    symbol: String,
    /// "stocks" or "etf". Empty until Nasdaq says which listing this symbol is.
    class: String,
    info: Option<Value>,
}

async fn resolve(http: &Client, input: &str, query: &str) -> Option<Hit> {
    let text = lookup_text(input, query);
    if text.is_empty() {
        return None;
    }
    // "TSLA" and "BRK.B" are tickers. "Tesla" and "tesla" are names — a
    // five-letter word like TESLA is not a symbol, so only an all-caps token
    // skips the search.
    let direct = (!text.contains(' '))
        .then(|| normalize_symbol(&text))
        .filter(|sym| is_ticker(sym));
    if single_token(input, query).is_some_and(|token| written_as_ticker(&token)) {
        if let Some(sym) = direct.clone() {
            if let Some((class, info)) = confirmed(http, &sym).await {
                return Some(Hit {
                    symbol: sym,
                    class: class.to_string(),
                    info: Some(info),
                });
            }
        }
    }
    if let Some((symbol, class)) = search_symbol(http, &text).await {
        return Some(Hit {
            symbol,
            class: class.to_string(),
            info: None,
        });
    }
    direct.map(|symbol| Hit {
        symbol,
        class: String::new(),
        info: None,
    })
}

async fn confirmed(http: &Client, sym: &str) -> Option<(&'static str, Value)> {
    for class in ["stocks", "etf"] {
        let Some(info) = nasdaq_kind(http, sym, class, "info", "").await else {
            continue;
        };
        let got = info.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        if normalize_symbol(got) == sym {
            return Some((class, info));
        }
    }
    None
}

async fn search_symbol(http: &Client, text: &str) -> Option<(String, &'static str)> {
    let q = encode_query(text);
    if q.is_empty() {
        return None;
    }
    let data = nasdaq(http, &format!("/api/autocomplete/slookup/10?search={q}")).await?;
    pick_symbol(data.as_array()?, text)
}

async fn assemble(http: &Client, hit: &Hit) -> Option<Quote> {
    let sym = &hit.symbol;
    let known = hit.info.clone();
    let prefer = hit.class.clone();
    let (info, summary, bars, tape, facts) = tokio::join!(
        async {
            if known.is_some() {
                known
            } else {
                nasdaq_listed(http, sym, "info", "", &prefer).await
            }
        },
        nasdaq_listed(http, sym, "summary", "", &prefer),
        nasdaq_listed(http, sym, "chart", "", &prefer),
        tape(http, sym),
        facts(http, sym, &prefer),
    );
    let points = bars
        .as_ref()
        .map(|bars| thin(chart_points(bars), 640))
        .unwrap_or_default();

    let price = tape
        .as_ref()
        .and_then(|t| num_of(&t["p"]))
        .or_else(|| info.as_ref().and_then(last_sale))
        .or_else(|| points.last().map(|p| p.p))
        .filter(|p| *p > 0.0)?;

    let previous = tape.as_ref().and_then(|t| num_of(&t["cl"])).or_else(|| {
        summary
            .as_ref()
            .and_then(|s| labeled(s, |l| l.contains("previous close")))
            .as_deref()
            .and_then(plain_num)
    });
    let change = tape
        .as_ref()
        .and_then(|t| num_of(&t["c"]))
        .or_else(|| {
            info.as_ref().and_then(|i| {
                i.pointer("/primaryData/netChange")
                    .and_then(|v| v.as_str())
                    .and_then(plain_num)
            })
        })
        .or_else(|| previous.map(|prev| price - prev));
    let change_percent = tape
        .as_ref()
        .and_then(|t| num_of(&t["cp"]))
        .or_else(|| {
            info.as_ref().and_then(|i| {
                i.pointer("/primaryData/percentageChange")
                    .and_then(|v| v.as_str())
                    .and_then(plain_num)
            })
        })
        .or_else(|| {
            previous
                .filter(|prev| *prev != 0.0)
                .zip(change)
                .map(|(prev, change)| change / prev * 100.0)
        });

    let day = summary
        .as_ref()
        .and_then(|s| labeled(s, |l| l.contains("today")))
        .as_deref()
        .and_then(low_high);
    let open = tape
        .as_ref()
        .and_then(|t| num_of(&t["o"]))
        .or_else(|| session_open(&points));
    let high = tape
        .as_ref()
        .and_then(|t| num_of(&t["h"]))
        .or_else(|| day.map(|(_, high)| high))
        .or_else(|| points.iter().map(|p| p.p).reduce(f64::max));
    let low = tape
        .as_ref()
        .and_then(|t| num_of(&t["l"]))
        .or_else(|| day.map(|(low, _)| low))
        .or_else(|| points.iter().map(|p| p.p).reduce(f64::min));
    let volume = tape
        .as_ref()
        .and_then(|t| num_of(&t["v"]))
        .or_else(|| {
            summary
                .as_ref()
                .and_then(|s| labeled(s, |l| l.contains("share volume")))
                .as_deref()
                .and_then(plain_num)
        })
        .or_else(|| {
            info.as_ref().and_then(|i| {
                i.pointer("/primaryData/volume")
                    .and_then(|v| v.as_str())
                    .and_then(plain_num)
            })
        });
    let (week_low, week_high) = tape
        .as_ref()
        .and_then(|t| Some((num_of(&t["l52"])?, num_of(&t["h52"])?)))
        .or_else(|| {
            summary
                .as_ref()
                .and_then(|s| labeled(s, |l| l.contains("52 week")))
                .as_deref()
                .and_then(low_high)
        })
        .map(|(low, high)| (Some(low), Some(high)))
        .unwrap_or((None, None));
    let market_cap = summary
        .as_ref()
        .and_then(|s| labeled(s, |l| l.contains("market cap")))
        .as_deref()
        .and_then(plain_num)
        .or(facts.market_cap);
    let dividend_yield = summary
        .as_ref()
        .and_then(|s| labeled(s, |l| l.contains("yield")))
        .as_deref()
        .and_then(yield_pct);

    let name = info
        .as_ref()
        .and_then(|i| i.get("companyName").and_then(|v| v.as_str()))
        .map(company_name)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sym.to_string());
    let exchange = tape
        .as_ref()
        .and_then(|t| t.get("ex").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            info.as_ref()
                .and_then(|i| i.get("exchange").and_then(|v| v.as_str()))
        })
        .map(exchange_label)
        .unwrap_or_default();
    let state = tape
        .as_ref()
        .and_then(|t| t.get("ms").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_ascii_lowercase();
    let session_label = match state.as_str() {
        "open" | "regular" => "Live",
        "pre" | "premarket" | "pre-market" => "Previous close",
        _ => "At close",
    };
    let as_of = tape
        .as_ref()
        .and_then(|t| millis_to_secs(&t["ts"]))
        .map(format_et)
        .unwrap_or_default();
    let extended = tape.as_ref().and_then(|t| extended_session(t, &state));

    Some(Quote {
        symbol: sym.to_string(),
        name,
        exchange,
        currency: "USD",
        timezone: "America/New_York",
        country: "US",
        price,
        change,
        change_percent,
        previous_close: previous,
        open,
        high,
        low,
        volume,
        market_cap,
        trailing_pe: facts.trailing_pe,
        forward_pe: facts.forward_pe,
        eps: facts.eps,
        week_low,
        week_high,
        dividend_yield,
        market_state: state,
        session_label: session_label.to_string(),
        as_of,
        extended,
        range: "1d",
        granularity: "minute",
        logo: format!(
            "https://logos.stockanalysis.com/{}.svg",
            sym.to_ascii_lowercase()
        ),
        points,
    })
}

fn extended_session(tape: &Value, state: &str) -> Option<Extended> {
    if state == "open" || state == "regular" {
        return None;
    }
    let price = num_of(&tape["ep"]).filter(|p| *p > 0.0)?;
    let mut label = tape
        .get("es")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if label.is_empty() {
        return None;
    }
    if let Some(rest) = label.strip_suffix(":") {
        label = rest.to_string();
    }
    Some(Extended {
        label,
        price,
        change: num_of(&tape["ec"]),
        change_percent: num_of(&tape["ecp"]),
        as_of: millis_to_secs(&tape["ets"])
            .map(format_et)
            .unwrap_or_default(),
    })
}

async fn load_points(http: &Client, sym: &str, range: &str) -> Vec<Point> {
    if range == "1d" {
        return intraday(http, sym).await;
    }
    daily(http, sym, range).await
}

async fn intraday(http: &Client, sym: &str) -> Vec<Point> {
    let Some(data) = nasdaq_listed(http, sym, "chart", "", "").await else {
        return Vec::new();
    };
    thin(chart_points(&data), 640)
}

async fn daily(http: &Client, sym: &str, range: &str) -> Vec<Point> {
    let today = market_today();
    let Some(from) = history_start(range, today) else {
        return Vec::new();
    };
    let query = format!(
        "&fromdate={}&todate={}&limit=2000",
        from.format("%Y-%m-%d"),
        today.format("%Y-%m-%d"),
    );
    let Some(data) = nasdaq_listed(http, sym, "historical", &query, "").await else {
        return Vec::new();
    };
    let mut points = history_points(&data);
    if range == "5d" && points.len() > 5 {
        points = points.split_off(points.len() - 5);
    }
    let cap = if range == "5y" { 420 } else { 640 };
    thin(points, cap)
}

/// Stocks and ETFs share these endpoints; the asset class is part of the path.
/// `prefer` is "stocks" or "etf" when the lookup already knows, and empty otherwise.
async fn nasdaq_listed(
    http: &Client,
    sym: &str,
    kind: &str,
    query: &str,
    prefer: &str,
) -> Option<Value> {
    let mut order = ["stocks", "etf"];
    if prefer == "etf" {
        order = ["etf", "stocks"];
    }
    for class in order {
        if let Some(data) = nasdaq_kind(http, sym, class, kind, query).await {
            return Some(data);
        }
    }
    None
}

async fn nasdaq_kind(
    http: &Client,
    sym: &str,
    class: &str,
    kind: &str,
    query: &str,
) -> Option<Value> {
    nasdaq(
        http,
        &format!("/api/quote/{sym}/{kind}?assetclass={class}{query}"),
    )
    .await
}

async fn nasdaq(http: &Client, path: &str) -> Option<Value> {
    let url = format!("https://api.nasdaq.com{path}");
    let v = fetch_json(http, &url, Some("https://www.nasdaq.com/")).await?;
    let data = v.get("data")?.clone();
    if data.is_null() { None } else { Some(data) }
}

async fn tape(http: &Client, sym: &str) -> Option<Value> {
    let url = format!("https://stockanalysis.com/api/quotes/s/{sym}");
    let v = fetch_json(http, &url, None).await?;
    let data = v.get("data")?.clone();
    if data.get("p").and_then(num_of).is_some() {
        Some(data)
    } else {
        None
    }
}

async fn facts(http: &Client, sym: &str, class: &str) -> Facts {
    let section = if class == "etf" { "etf" } else { "stocks" };
    let url = format!(
        "https://stockanalysis.com/{section}/{}/__data.json",
        sym.to_ascii_lowercase()
    );
    fetch_json(http, &url, None)
        .await
        .as_ref()
        .map(read_facts)
        .unwrap_or(Facts {
            trailing_pe: None,
            forward_pe: None,
            eps: None,
            market_cap: None,
        })
}

async fn fetch_json(http: &Client, url: &str, referer: Option<&str>) -> Option<Value> {
    let mut req = http
        .get(url)
        .header("user-agent", BROWSER)
        .header("accept", "application/json")
        .header("accept-language", "en-US,en;q=0.9")
        .timeout(std::time::Duration::from_secs(8));
    if let Some(referer) = referer {
        req = req
            .header("referer", referer)
            .header("origin", "https://www.nasdaq.com");
    }
    let res = req.send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    res.json().await.ok()
}

fn canonical_range(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1d" | "1" => Some("1d"),
        "5d" => Some("5d"),
        "1m" | "1mo" => Some("1m"),
        "6m" | "6mo" => Some("6m"),
        "ytd" => Some("ytd"),
        "1y" => Some("1y"),
        "5y" => Some("5y"),
        _ => None,
    }
}

fn history_start(range: &str, today: NaiveDate) -> Option<NaiveDate> {
    if range == "ytd" {
        return NaiveDate::from_ymd_opt(today.year(), 1, 1);
    }
    let days = match range {
        "5d" => 14,
        "1m" => 35,
        "6m" => 190,
        "1y" => 372,
        "5y" => 365 * 5 + 10,
        _ => return None,
    };
    today.checked_sub_signed(chrono::Duration::days(days))
}

fn market_today() -> NaiveDate {
    Utc::now().with_timezone(&New_York).date_naive()
}

/// Nasdaq's chart `x` is often the New York clock stored as if it were UTC
/// ("4:00 AM ET" arrives as 04:00Z). When the label already matches a real
/// New York reading of `x`, the feed has been corrected and `x` is used as-is.
fn point_unix(x: i64, label: &str) -> i64 {
    let secs = if x.abs() > 10_000_000_000 {
        x / 1000
    } else {
        x
    };
    let shifted = clock_as_new_york(secs);
    let Some(hour) = label_hour(label) else {
        return shifted;
    };
    if hour_in_new_york(secs) == Some(hour) {
        secs
    } else if hour_in_new_york(shifted) == Some(hour) {
        shifted
    } else {
        shifted
    }
}

fn clock_as_new_york(secs: i64) -> i64 {
    let Some(utc) = DateTime::<Utc>::from_timestamp(secs, 0) else {
        return secs;
    };
    match New_York.from_local_datetime(&utc.naive_utc()) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        LocalResult::None => secs,
    }
}

fn hour_in_new_york(secs: i64) -> Option<u32> {
    New_York.timestamp_opt(secs, 0).single().map(|dt| dt.hour())
}

fn label_hour(label: &str) -> Option<u32> {
    let mut parts = label.split_whitespace();
    let clock = parts.next()?;
    let ampm = parts.next()?.to_ascii_uppercase();
    let hour: u32 = clock.split(':').next()?.parse().ok()?;
    match ampm.as_str() {
        "AM" if hour == 12 => Some(0),
        "AM" => Some(hour),
        "PM" if hour == 12 => Some(12),
        "PM" => Some(hour + 12),
        _ => None,
    }
}

fn chart_points(data: &Value) -> Vec<Point> {
    let Some(rows) = data.get("chart").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(p) = row.get("y").and_then(num_of) else {
            continue;
        };
        let Some(x) = row.get("x").and_then(int_of) else {
            continue;
        };
        let label = row
            .get("z")
            .and_then(|z| z.get("dateTime"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        out.push(Point {
            t: point_unix(x, label),
            p,
            v: None,
        });
    }
    drop_spikes(dedupe(out))
}

fn history_points(data: &Value) -> Vec<Point> {
    let Some(rows) = data.pointer("/tradesTable/rows").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(close) = row
            .get("close")
            .and_then(|v| v.as_str())
            .and_then(plain_num)
        else {
            continue;
        };
        let Some(t) = row
            .get("date")
            .and_then(|v| v.as_str())
            .and_then(close_unix)
        else {
            continue;
        };
        let v = row
            .get("volume")
            .and_then(|v| v.as_str())
            .and_then(plain_num);
        out.push(Point { t, p: close, v });
    }
    dedupe(out)
}

fn close_unix(mdy: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(mdy.trim(), "%m/%d/%Y").ok()?;
    let naive = date.and_hms_opt(16, 0, 0)?;
    match New_York.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Some(dt.timestamp()),
        LocalResult::None => None,
    }
}

fn session_open(points: &[Point]) -> Option<f64> {
    points
        .iter()
        .find(|p| {
            New_York
                .timestamp_opt(p.t, 0)
                .single()
                .is_some_and(|dt| dt.hour() * 60 + dt.minute() >= 9 * 60 + 30)
        })
        .map(|p| p.p)
}

fn format_et(secs: i64) -> String {
    let Some(dt) = New_York.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    let hour24 = dt.hour();
    let hour = match hour24 % 12 {
        0 => 12,
        h => h,
    };
    let ampm = if hour24 < 12 { "AM" } else { "PM" };
    format!(
        "{} {}, {hour}:{:02}:{:02} {ampm} {}",
        dt.format("%b"),
        dt.day(),
        dt.minute(),
        dt.second(),
        dt.format("%Z"),
    )
}

fn read_facts(v: &Value) -> Facts {
    let empty = Facts {
        trailing_pe: None,
        forward_pe: None,
        eps: None,
        market_cap: None,
    };
    let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) else {
        return empty;
    };
    for node in nodes {
        let Some(data) = node.get("data").and_then(|d| d.as_array()) else {
            continue;
        };
        let Some(root) = data.first().and_then(|d| d.as_object()) else {
            continue;
        };
        if !root.contains_key("peRatio")
            && !root.contains_key("forwardPE")
            && !root.contains_key("eps")
        {
            continue;
        }
        let get = |key: &str| -> Option<f64> {
            let slot = root.get(key)?;
            let resolved = match slot.as_u64() {
                Some(i) => data.get(i as usize)?,
                None => slot,
            };
            match resolved {
                Value::String(s) => plain_num(s).or_else(|| suffixed(s)),
                other => num_of(other),
            }
        };
        return Facts {
            trailing_pe: get("peRatio"),
            forward_pe: get("forwardPE"),
            eps: get("eps"),
            market_cap: get("marketCap"),
        };
    }
    empty
}

fn labeled(summary: &Value, pred: impl Fn(&str) -> bool) -> Option<String> {
    let obj = summary.get("summaryData")?.as_object()?;
    for v in obj.values() {
        let label = v.get("label")?.as_str()?.to_ascii_lowercase();
        if pred(&label) {
            return v.get("value")?.as_str().map(str::to_string);
        }
    }
    None
}

fn last_sale(info: &Value) -> Option<f64> {
    info.pointer("/primaryData/lastSalePrice")
        .and_then(|v| v.as_str())
        .and_then(plain_num)
}

fn lookup_text(input: &str, query: &str) -> String {
    let raw = if input.trim().is_empty() {
        query
    } else {
        input
    };
    const DROP: &[&str] = &[
        "a",
        "an",
        "the",
        "what",
        "whats",
        "what's",
        "is",
        "are",
        "of",
        "for",
        "how",
        "much",
        "does",
        "do",
        "did",
        "price",
        "prices",
        "quote",
        "quotes",
        "stock",
        "stocks",
        "share",
        "shares",
        "today",
        "now",
        "please",
        "show",
        "me",
        "current",
        "latest",
        "trading",
        "at",
        "in",
        "on",
        "about",
        "cost",
        "costs",
        "worth",
        "inc",
        "incorporated",
        "corp",
        "corporation",
        "company",
        "co",
        "ltd",
        "plc",
        "and",
        "usd",
        "dollar",
        "dollars",
    ];
    let mut words = Vec::new();
    for raw_word in raw.split_whitespace() {
        let trimmed = raw_word
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '&');
        let word = trimmed
            .trim_end_matches("'s")
            .trim_end_matches("’s")
            .replace(['\'', '’'], "")
            .to_lowercase();
        if word.is_empty() || DROP.contains(&word.as_str()) {
            continue;
        }
        words.push(word);
    }
    words.join(" ")
}

fn normalize_symbol(raw: &str) -> String {
    let s = raw.trim().to_ascii_uppercase().replace('/', ".");
    if let Some((head, tail)) = s.split_once('-') {
        if (1..=5).contains(&head.len())
            && head.chars().all(|c| c.is_ascii_uppercase())
            && tail.len() == 1
            && tail.chars().all(|c| c.is_ascii_uppercase())
        {
            return format!("{head}.{tail}");
        }
    }
    s
}

fn single_token(input: &str, query: &str) -> Option<String> {
    let raw = if input.trim().is_empty() {
        query
    } else {
        input
    };
    let mut parts = raw.split_whitespace();
    let token = parts
        .next()?
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
    if parts.next().is_some() || token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// An all-caps token the user typed as a symbol, not a company name.
fn written_as_ticker(token: &str) -> bool {
    is_ticker(&normalize_symbol(token))
        && token.chars().any(|c| c.is_ascii_uppercase())
        && token.chars().all(|c| !c.is_ascii_lowercase())
}

fn is_ticker(sym: &str) -> bool {
    let mut parts = sym.split('.');
    let Some(head) = parts.next() else {
        return false;
    };
    let tail = parts.next();
    if parts.next().is_some() {
        return false;
    }
    let head_ok = (1..=5).contains(&head.len()) && head.chars().all(|c| c.is_ascii_uppercase());
    let tail_ok = match tail {
        None => true,
        Some(t) => t.len() == 1 && t.chars().all(|c| c.is_ascii_uppercase()),
    };
    head_ok && tail_ok
}

fn asset_class(asset: &str) -> Option<&'static str> {
    match asset.trim().to_ascii_lowercase().as_str() {
        "stock" | "stocks" => Some("stocks"),
        "etf" | "etfs" => Some("etf"),
        _ => None,
    }
}

fn pick_symbol(hits: &[Value], query: &str) -> Option<(String, &'static str)> {
    let want = normalize_symbol(query);
    let listed: Vec<&Value> = hits
        .iter()
        .filter(|h| {
            h.get("asset")
                .and_then(|v| v.as_str())
                .and_then(asset_class)
                .is_some()
        })
        .collect();
    let stocks: Vec<&Value> = listed
        .iter()
        .copied()
        .filter(|h| {
            h.get("asset")
                .and_then(|v| v.as_str())
                .and_then(asset_class)
                == Some("stocks")
        })
        .collect();
    // An exact ticker wins across stocks and ETFs. A company name falls
    // through to the common stock.
    let exact = listed.iter().copied().find(|h| {
        h.get("symbol")
            .and_then(|v| v.as_str())
            .is_some_and(|s| normalize_symbol(s) == want)
    });
    let pool: Vec<&Value> = if stocks.is_empty() { listed } else { stocks };
    let hit = exact.or_else(|| pool.first().copied())?;
    let sym = normalize_symbol(hit.get("symbol")?.as_str()?);
    let class = asset_class(hit.get("asset")?.as_str()?)?;
    is_ticker(&sym).then_some((sym, class))
}

fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '-' => out.push(c),
            '&' => out.push_str("%26"),
            ' ' => out.push_str("%20"),
            _ => {}
        }
    }
    out
}

fn company_name(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    const SUFFIXES: &[&str] = &[
        " Class A Common Stock",
        " Class B Common Stock",
        " Class C Common Stock",
        " New Common Stock",
        " Common Stock",
        " Common Shares",
        " Ordinary Shares",
        " Capital Stock",
        " American Depositary Shares",
    ];
    loop {
        let next = SUFFIXES.iter().find_map(|suf| s.strip_suffix(suf));
        let Some(rest) = next else { break };
        s = rest.trim().to_string();
    }
    if let Some(rest) = s.strip_suffix(" (The)") {
        s = format!("The {}", rest.trim());
    }
    s
}

fn exchange_label(raw: &str) -> String {
    let u = raw.trim().to_ascii_uppercase();
    if u.starts_with("NASDAQ") {
        "NASDAQ".to_string()
    } else if u.contains("AMEX") || u.contains("AMERICAN") {
        "NYSE American".to_string()
    } else if u.starts_with("NYSE") {
        "NYSE".to_string()
    } else {
        raw.trim().to_string()
    }
}

fn plain_num(raw: &str) -> Option<f64> {
    let t = raw
        .trim()
        .replace('−', "-")
        .replace(['$', ',', ' ', '+', '%'], "");
    if t.is_empty()
        || t.eq_ignore_ascii_case("n/a")
        || t.eq_ignore_ascii_case("na")
        || t == "-"
        || t == "—"
    {
        return None;
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn suffixed(raw: &str) -> Option<f64> {
    let t = raw.trim().trim_start_matches('$').replace(',', "");
    let (n, mult) = if let Some(x) = t.strip_suffix(['T', 't']) {
        (x, 1e12)
    } else if let Some(x) = t.strip_suffix(['B', 'b']) {
        (x, 1e9)
    } else if let Some(x) = t.strip_suffix(['M', 'm']) {
        (x, 1e6)
    } else {
        return None;
    };
    Some(n.trim().parse::<f64>().ok()? * mult)
}

fn yield_pct(raw: &str) -> Option<f64> {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("n/a") || t == "-" || t == "—" {
        return None;
    }
    let percent = t.ends_with('%');
    let n = plain_num(t)?;
    if !percent && n.abs() < 0.25 {
        Some(n * 100.0)
    } else {
        Some(n)
    }
}

fn low_high(raw: &str) -> Option<(f64, f64)> {
    let t = raw.replace(['$', ','], "");
    let parts: Vec<&str> = if t.contains('/') {
        t.split('/').collect()
    } else {
        t.split('-')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect()
    };
    if parts.len() != 2 {
        return None;
    }
    let a = plain_num(parts[0])?;
    let b = plain_num(parts[1])?;
    Some((a.min(b), a.max(b)))
}

fn num_of(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .filter(|n| n.is_finite())
}

fn int_of(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| num_of(v).map(|n| n.round() as i64))
}

fn millis_to_secs(v: &Value) -> Option<i64> {
    let n = num_of(v)?;
    if n > 1e12 {
        Some((n / 1000.0).round() as i64)
    } else if n > 1e9 {
        Some(n.round() as i64)
    } else {
        None
    }
}

fn dedupe(mut points: Vec<Point>) -> Vec<Point> {
    points.sort_by_key(|p| p.t);
    points.dedup_by_key(|p| p.t);
    points
}

/// Nasdaq's minute chart occasionally prints one bar far from both neighbors.
/// A print that fully reverts, while the bars on either side agree, is dropped.
fn drop_spikes(points: Vec<Point>) -> Vec<Point> {
    if points.len() < 3 {
        return points;
    }
    let mut drop = vec![false; points.len()];
    for i in 1..points.len() - 1 {
        let (a, b, c) = (points[i - 1].p, points[i].p, points[i + 1].p);
        if a <= 0.0 || b <= 0.0 || c <= 0.0 {
            continue;
        }
        let neighbors = (a - c).abs() / a.max(c);
        let from_a = (b - a).abs() / a;
        let from_c = (b - c).abs() / c;
        if neighbors < 0.005 && from_a > 0.006 && from_c > 0.006 {
            drop[i] = true;
        }
    }
    points
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| (!drop[i]).then_some(p))
        .collect()
}

fn thin(points: Vec<Point>, cap: usize) -> Vec<Point> {
    if cap < 2 || points.len() <= cap {
        return points;
    }
    let last = points.len() - 1;
    let step = last as f64 / (cap - 1) as f64;
    let mut out = Vec::with_capacity(cap);
    let mut prev = usize::MAX;
    for i in 0..cap {
        let idx = if i + 1 == cap {
            last
        } else {
            (i as f64 * step).round() as usize
        };
        if idx != prev {
            out.push(points[idx].clone());
            prev = idx;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_lookups() {
        assert_eq!(company_name("Tesla, Inc. Common Stock"), "Tesla, Inc.");
        assert_eq!(
            company_name("Coca-Cola Company (The) Common Stock"),
            "The Coca-Cola Company"
        );
        assert_eq!(
            company_name("Berkshire Hathaway Inc. New Common Stock"),
            "Berkshire Hathaway Inc."
        );
        assert_eq!(lookup_text("TSLA", "what is apple stock"), "tsla");
        assert_eq!(lookup_text("", "what's the tesla stock price?"), "tesla");
        assert_eq!(lookup_text("Tesla Inc", ""), "tesla");
        assert_eq!(normalize_symbol("brk/b"), "BRK.B");
        assert_eq!(normalize_symbol("brk-b"), "BRK.B");
        assert!(is_ticker("TSLA"));
        assert!(is_ticker("BRK.B"));
        assert!(is_ticker("TESLA"));
        assert!(!is_ticker("COCA-COLA"));
        assert!(written_as_ticker("TSLA"));
        assert!(written_as_ticker("BRK.B"));
        assert!(!written_as_ticker("Tesla"));
        assert!(!written_as_ticker("tesla"));
        assert!(!written_as_ticker("Ford"));
        assert_eq!(exchange_label("NASDAQ-GS"), "NASDAQ");
        assert_eq!(exchange_label("NYSE"), "NYSE");
    }

    #[test]
    fn parses_quote_figures() {
        assert_eq!(plain_num("+$7.70"), Some(7.7));
        assert_eq!(plain_num("38,953,094"), Some(38_953_094.0));
        assert_eq!(plain_num("N/A"), None);
        assert_eq!(low_high("$498.83/$297.38"), Some((297.38, 498.83)));
        assert_eq!(low_high("$88.4750/$87.3915"), Some((87.3915, 88.475)));
        assert_eq!(yield_pct("N/A"), None);
        assert_eq!(yield_pct("2.85%"), Some(2.85));
        assert_eq!(yield_pct("0.0285"), Some(2.85));
        assert_eq!(suffixed("1.51T"), Some(1.51e12));
    }

    #[test]
    fn drops_a_minute_print_that_fully_reverts() {
        let points = vec![
            Point {
                t: 1,
                p: 382.45,
                v: None,
            },
            Point {
                t: 2,
                p: 368.51,
                v: None,
            },
            Point {
                t: 3,
                p: 382.46,
                v: None,
            },
            Point {
                t: 4,
                p: 379.83,
                v: None,
            },
            Point {
                t: 5,
                p: 382.58,
                v: None,
            },
            Point {
                t: 6,
                p: 381.5,
                v: None,
            },
            Point {
                t: 7,
                p: 382.4,
                v: None,
            },
        ];
        let kept: Vec<_> = drop_spikes(points).into_iter().map(|p| p.t).collect();
        assert_eq!(kept, vec![1, 3, 5, 6, 7]);
    }

    #[test]
    fn chart_clock_matches_the_exchange_label() {
        // "4:00 AM ET" published as 04:00Z.
        assert_eq!(point_unix(1_791_518_400_000, "4:00 AM ET"), 1_791_532_800);
        // A timestamp that is already 4:00 AM in New York stays put.
        assert_eq!(point_unix(1_791_532_800_000, "4:00 AM ET"), 1_791_532_800);
        assert_eq!(format_et(1_791_576_000), "Oct 9, 4:00:00 PM EDT");
    }

    #[test]
    fn prefers_the_listed_stock() {
        let hits = json!([
            {"symbol": "BFOCX", "asset": "MUTUALFUNDS", "name": "Berkshire Focus Fund"},
            {"symbol": "BRK/A", "asset": "STOCKS", "name": "Berkshire Hathaway Inc."},
            {"symbol": "BRK.B", "asset": "STOCKS", "name": "Berkshire Hathaway Inc."}
        ]);
        assert_eq!(
            pick_symbol(hits.as_array().unwrap(), "berkshire"),
            Some(("BRK.A".to_string(), "stocks"))
        );
        assert_eq!(
            pick_symbol(hits.as_array().unwrap(), "brk.b"),
            Some(("BRK.B".to_string(), "stocks"))
        );
        let etf = json!([
            {"symbol": "SPYGX", "asset": "MUTUALFUNDS", "name": "Spyglass Growth Fund"},
            {"symbol": "SPY", "asset": "ETF", "name": "State Street SPDR S&P 500 ETF Trust"}
        ]);
        assert_eq!(
            pick_symbol(etf.as_array().unwrap(), "spy"),
            Some(("SPY".to_string(), "etf"))
        );
    }

    #[test]
    fn reads_indexed_fundamentals_and_daily_bars() {
        let facts = read_facts(&json!({"nodes":[{"data":[
            {"peRatio":1,"forwardPE":2,"eps":3,"marketCap":4},
            "397.13","196.14","1.08","1.51T"
        ]}]}));
        assert_eq!(facts.trailing_pe, Some(397.13));
        assert_eq!(facts.forward_pe, Some(196.14));
        assert_eq!(facts.eps, Some(1.08));
        assert_eq!(facts.market_cap, Some(1.51e12));

        let bars = history_points(&json!({"tradesTable":{"rows":[
            {"date":"10/09/2026","close":"$382.70","volume":"38,952,430"},
            {"date":"10/08/2026","close":"$375.00","volume":"1,000"}
        ]}}));
        assert_eq!(bars.len(), 2);
        assert!(bars[0].t < bars[1].t);
        assert_eq!(bars[1].p, 382.70);
        assert_eq!(bars[1].v, Some(38_952_430.0));
    }

    #[test]
    fn summary_labels_are_read_by_name() {
        let summary = json!({"summaryData":{
            "MarketCap":{"label":"Market Cap","value":"1,511,491,787,684"},
            "PreviousClose":{"label":"Previous Close","value":"$375.00"},
            "Yield":{"label":"Current Yield","value":"N/A"}
        }});
        assert_eq!(
            labeled(&summary, |l| l.contains("market cap"))
                .as_deref()
                .and_then(plain_num),
            Some(1_511_491_787_684.0)
        );
        assert_eq!(
            labeled(&summary, |l| l.contains("yield"))
                .as_deref()
                .and_then(yield_pct),
            None
        );
    }

    #[tokio::test]
    async fn rejects_an_unknown_range() {
        let http = reqwest::Client::new();
        let err = chart(&http, "TSLA", "2w").await.unwrap_err();
        assert!(matches!(err, ChartError::Rejected));
        let err = chart(&http, "not a ticker", "1d").await.unwrap_err();
        assert!(matches!(err, ChartError::Rejected));
    }

    #[tokio::test]
    #[ignore = "hits Nasdaq and StockAnalysis"]
    async fn live_tesla_quote() {
        let http = reqwest::Client::new();
        let block = quote(&http, "tesla", "tesla stock price")
            .await
            .expect("quote");
        let data = block.data.unwrap();
        assert_eq!(data["symbol"], "TSLA");
        assert!(data["name"].as_str().unwrap().contains("Tesla"));
        assert!(data["price"].as_f64().unwrap() > 50.0);
        assert!(data["market_cap"].as_f64().unwrap() > 1e11);
        let points = data["points"].as_array().unwrap();
        assert!(points.len() > 30);
        let first = points[0]["t"].as_i64().unwrap();
        let opened = chrono_tz::America::New_York
            .timestamp_opt(first, 0)
            .single()
            .expect("first bar");
        assert!(
            (4..12).contains(&chrono::Timelike::hour(&opened)),
            "intraday should open in the morning, got {}",
            format_et(first)
        );
        let series = chart(&http, "TSLA", "5d").await.expect("5d");
        assert_eq!(series["granularity"], "day");
        assert!(series["points"].as_array().unwrap().len() >= 4);
    }

    #[tokio::test]
    #[ignore = "hits Nasdaq and StockAnalysis"]
    async fn live_spy_etf_has_a_chart() {
        let http = reqwest::Client::new();
        let block = quote(&http, "SPY", "SPY").await.expect("quote");
        let data = block.data.unwrap();
        assert_eq!(data["symbol"], "SPY");
        assert!(
            data["name"]
                .as_str()
                .unwrap()
                .to_ascii_lowercase()
                .contains("spdr")
                || data["name"].as_str().unwrap().eq_ignore_ascii_case("spy")
        );
        assert!(data["points"].as_array().unwrap().len() > 30);
        let series = chart(&http, "SPY", "5d").await.expect("5d");
        assert!(series["points"].as_array().unwrap().len() >= 4);
    }
}
