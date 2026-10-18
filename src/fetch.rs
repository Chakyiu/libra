//! Read the pages an answer will cite.
//!
//! Obscura runs the page's scripts and, in a stealth build, fetches with a
//! browser-like TLS fingerprint. When that binary is missing, a plain HTTP GET
//! still extracts the text of a static document. Private and loopback addresses
//! are refused either way: a question can contain any URL.

use crate::config::Config;
use crate::search::types::SearchResult;
use regex::Regex;
use reqwest::redirect::Policy;
use scraper::Selector;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use url::Url;

const PAGE_CHARS: usize = 6_000;
const SNIPPET_CHARS: usize = 280;
const ALREADY_READ: usize = 800;
const MIN_BODY: usize = 40;
const DOCUMENT_CHARS: usize = 500;
const MAX_USER_URLS: usize = 4;
const PARALLEL: usize = 4;
const HTTP_BYTES: usize = 1_500_000;
const OBSCURA_BYTES: usize = 96 * 1024;

const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

static MISSING_OBSCURA: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageBody {
    pub url: String,
    pub title: String,
    pub text: String,
}

/// `"obscura"` when the configured binary is on disk, `"http"` when page reading
/// falls back to a plain request, `"off"` when reading is disabled.
pub fn reader_kind(cfg: &Config) -> &'static str {
    if !cfg.read_pages {
        return "off";
    }
    if resolve_bin(&cfg.obscura_bin).is_some() {
        "obscura"
    } else {
        "http"
    }
}

/// http(s) links typed in the question, in order, with private targets removed.
pub fn user_urls(texts: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for text in texts {
        for raw in extract_http_urls(text) {
            let Some(url) = public_http_url(&raw) else {
                continue;
            };
            if !seen.insert(page_key(&url)) {
                continue;
            }
            out.push(url);
            if out.len() == MAX_USER_URLS {
                return out;
            }
        }
    }
    out
}

/// User links first, then cited sources that are still only a snippet.
pub fn plan_reads(user_urls: &[String], results: &[SearchResult], budget: usize) -> Vec<String> {
    let budget = budget.clamp(1, 12);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for url in user_urls.iter().take(MAX_USER_URLS) {
        let Some(url) = public_http_url(url) else {
            continue;
        };
        if results
            .iter()
            .any(|r| page_key(&r.url) == page_key(&url) && already_read(r))
        {
            continue;
        }
        if asset_url(&url) || !seen.insert(page_key(&url)) {
            continue;
        }
        out.push(url);
    }
    let cap = budget.max(out.len()).min(12);
    for result in results {
        if out.len() >= cap {
            break;
        }
        if already_read(result) || asset_url(&result.url) {
            continue;
        }
        let Some(url) = public_http_url(&result.url) else {
            continue;
        };
        if !seen.insert(page_key(&url)) {
            continue;
        }
        out.push(url);
    }
    out
}

/// Fetch `plan_reads` and attach the text to the matching source. Links from the
/// question that were not in the search results are added at the front.
pub async fn enrich(
    cfg: &Config,
    user_urls: &[String],
    results: Vec<SearchResult>,
    budget: usize,
    cancel: &AtomicBool,
) -> Vec<SearchResult> {
    if !cfg.read_pages {
        return results;
    }
    let targets = plan_reads(user_urls, &results, budget);
    if targets.is_empty() {
        return results;
    }
    let mut pages = Vec::with_capacity(targets.len());
    for chunk in targets.chunks(PARALLEL) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let batch = chunk
            .iter()
            .map(|url| read_one(cfg, url, cancel))
            .collect::<Vec<_>>();
        for page in futures_util::future::join_all(batch)
            .await
            .into_iter()
            .flatten()
        {
            pages.push(page);
        }
    }
    apply_pages(results, user_urls, &pages)
}

pub fn apply_pages(
    mut results: Vec<SearchResult>,
    user_urls: &[String],
    pages: &[PageBody],
) -> Vec<SearchResult> {
    let by_key: HashMap<String, &PageBody> = pages
        .iter()
        .map(|page| (page_key(&page.url), page))
        .collect();
    for result in &mut results {
        let Some(page) = by_key.get(&page_key(&result.url)) else {
            continue;
        };
        fill_result(result, page);
    }

    let mut used = HashSet::new();
    let mut front = Vec::new();
    for url in user_urls {
        let key = page_key(url);
        if !used.insert(key.clone()) {
            continue;
        }
        if let Some(pos) = results.iter().position(|r| page_key(&r.url) == key) {
            front.push(results.remove(pos));
            continue;
        }
        if let Some(page) = by_key.get(&key) {
            front.push(result_from_page(page));
        }
    }
    front.extend(results);
    front
}

fn fill_result(result: &mut SearchResult, page: &PageBody) {
    if result.title.trim().is_empty() {
        result.title = page.title.clone();
    }
    if result.snippet.trim().is_empty() {
        result.snippet = clip(&squash(&page.text), SNIPPET_CHARS);
    }
    result.content = Some(clip(&page.text, PAGE_CHARS));
}

fn result_from_page(page: &PageBody) -> SearchResult {
    SearchResult {
        title: page.title.clone(),
        url: page.url.clone(),
        snippet: clip(&squash(&page.text), SNIPPET_CHARS),
        content: Some(clip(&page.text, PAGE_CHARS)),
        source: "page".into(),
        ..SearchResult::default()
    }
}

async fn read_one(cfg: &Config, url: &str, cancel: &AtomicBool) -> Option<PageBody> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let url = public_http_url(url)?;
    if !host_resolves_public(&url).await {
        tracing::info!(url = %url, "skipped page read; host is not a public address");
        return None;
    }
    // A server-rendered article is already in the HTML. Obscura is for a thin or
    // blocked response. Its rendered "allow ads" screen must not replace the article.
    let http_page = read_http(cfg, &url).await;
    if http_page
        .as_ref()
        .is_some_and(|page| is_document(&page.text))
    {
        let chars = http_page.as_ref().map(|page| page.text.chars().count());
        tracing::info!(url = %url, via = "http", ?chars, "read page");
        return http_page;
    }
    if cancel.load(Ordering::Relaxed) {
        return http_page.filter(|page| usable(&page.text));
    }
    if let Some(page) = read_obscura(cfg, &url).await {
        if usable(&page.text) {
            tracing::info!(url = %url, via = "obscura", chars = page.text.chars().count(), "read page");
            return Some(page);
        }
    }
    let page = http_page?;
    if !usable(&page.text) {
        return None;
    }
    tracing::info!(url = %url, via = "http", chars = page.text.chars().count(), "read page");
    Some(page)
}

async fn read_obscura(cfg: &Config, url: &str) -> Option<PageBody> {
    let Some(bin) = resolve_bin(&cfg.obscura_bin) else {
        if !MISSING_OBSCURA.swap(true, Ordering::Relaxed) {
            tracing::warn!(bin = %cfg.obscura_bin, "obscura not found; reading pages over HTTP");
        }
        return None;
    };
    let timeout = Duration::from_secs(cfg.fetch_timeout_secs.clamp(5, 60) as u64 + 5);
    let bin = bin.to_string_lossy().to_string();
    let attempts = [
        obscura_args(cfg.obscura_stealth, true, cfg.fetch_timeout_secs, url),
        obscura_args(false, false, cfg.fetch_timeout_secs, url),
    ];
    for (i, args) in attempts.iter().enumerate() {
        let (out, err) = run_command(&bin, args, timeout).await?;
        if usable(&out) {
            return Some(page_from_text(url, &out));
        }
        // A binary without stealth or --quiet exits immediately. Anything else is the page.
        if i == 0 && !flag_rejected(&err) {
            break;
        }
    }
    None
}

async fn run_command(bin: &str, args: &[String], timeout: Duration) -> Option<(String, String)> {
    let mut child = match Command::new(bin)
        .args(args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("OBSCURA_ALLOW_PRIVATE_NETWORK", "0")
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            if MISSING_OBSCURA.swap(true, Ordering::Relaxed) {
                tracing::debug!(bin, error = %e, "obscura did not start");
            } else {
                tracing::warn!(bin, error = %e, "obscura did not start; reading pages over HTTP");
            }
            return None;
        }
    };
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let work = async move {
        let (out, err) = tokio::join!(
            read_capped(&mut stdout, OBSCURA_BYTES),
            read_capped(&mut stderr, 16 * 1024),
        );
        let _ = child.wait().await;
        (out, err)
    };
    let (out, err) = tokio::time::timeout(timeout, work).await.ok()?;
    Some((
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    ))
}

async fn read_http(cfg: &Config, url: &str) -> Option<PageBody> {
    let timeout = Duration::from_secs(cfg.fetch_timeout_secs.clamp(5, 60) as u64);
    let response = http_client()
        .get(url)
        .timeout(timeout)
        .header("accept", "text/html,application/xhtml+xml,text/plain;q=0.9")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let kind = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(kind.is_empty()
        || kind.contains("html")
        || kind.contains("text/plain")
        || kind.contains("markdown"))
    {
        return None;
    }
    let bytes = read_body(response, HTTP_BYTES).await?;
    let html = String::from_utf8_lossy(&bytes);
    let (title, text) = if kind.contains("text/plain") || kind.contains("markdown") {
        (title_from_text(&html, url), tidy(&html))
    } else {
        html_to_text(&html, url)
    };
    if !usable(&text) {
        return None;
    }
    Some(PageBody {
        url: url.to_string(),
        title,
        text,
    })
}

async fn read_body(mut response: reqwest::Response, cap: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    while buf.len() < cap {
        let chunk = response.chunk().await.ok()??;
        let room = cap - buf.len();
        if chunk.len() > room {
            buf.extend_from_slice(&chunk[..room]);
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    Some(buf)
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .redirect(Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.stop();
                }
                let Some(url) = public_http_url(attempt.url().as_str()) else {
                    return attempt.stop();
                };
                let Some(host) = Url::parse(&url)
                    .ok()
                    .and_then(|parsed| parsed.host_str().map(str::to_string))
                else {
                    return attempt.stop();
                };
                if host_resolves_public_blocking(&host) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("page reader client")
    })
}

fn page_from_text(url: &str, text: &str) -> PageBody {
    let text = tidy(text);
    PageBody {
        url: url.to_string(),
        title: title_from_text(&text, url),
        text,
    }
}

fn usable(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.chars().count() >= MIN_BODY && !is_challenge(trimmed)
}

fn is_document(text: &str) -> bool {
    usable(text) && text.trim().chars().count() >= DOCUMENT_CHARS
}

fn is_challenge(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(1_500)
        .collect::<String>()
        .to_ascii_lowercase();
    // Bot checks, and the "allow ads" screen sites show when a headless browser
    // fails their script integrity check. The article is still in the raw HTML.
    (head.contains("just a moment") && head.contains("cloudflare"))
        || head.contains("cf-browser-verification")
        || (head.contains("attention required") && head.contains("cloudflare"))
        || (head.contains("access denied") && head.contains("akamai"))
        || head.contains("please allow ads")
        || head.contains("our site is supported by ads")
        || head.contains("allow ads to continue")
        || head.contains("integrity check failed")
}

fn already_read(result: &SearchResult) -> bool {
    result
        .content
        .as_deref()
        .is_some_and(|text| text.trim().chars().count() >= ALREADY_READ)
}

fn asset_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return true;
    };
    let path = parsed.path().to_ascii_lowercase();
    const SKIP: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".bmp", ".avif", ".mp4", ".webm",
        ".mp3", ".wav", ".zip", ".gz", ".tgz", ".css", ".js", ".mjs", ".woff", ".woff2", ".ttf",
        ".eot",
    ];
    SKIP.iter().any(|ext| path.ends_with(ext))
}

pub(crate) fn obscura_args(
    stealth: bool,
    quiet: bool,
    timeout_secs: usize,
    url: &str,
) -> Vec<String> {
    let mut args = Vec::new();
    if stealth {
        args.push("--stealth".into());
    }
    args.push("fetch".into());
    args.extend([
        "--dump".into(),
        "markdown".into(),
        "--timeout".into(),
        timeout_secs.clamp(5, 60).to_string(),
    ]);
    if quiet {
        args.push("--quiet".into());
    }
    args.push("--".into());
    args.push(url.to_string());
    args
}

fn flag_rejected(err: &str) -> bool {
    let err = err.to_ascii_lowercase();
    err.contains("unexpected") || err.contains("unrecognized") || err.contains("unknown argument")
}

fn resolve_bin(bin: &str) -> Option<PathBuf> {
    let bin = bin.trim();
    if bin.is_empty() {
        return None;
    }
    let path = Path::new(bin);
    if path.components().nth(1).is_some() || bin.contains('/') || bin.contains('\\') {
        return path.is_file().then(|| path.to_path_buf());
    }
    let dirs = std::env::var_os("PATH")?;
    let dirs = dirs.to_string_lossy();
    for dir in dirs.split([':', ';']) {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

async fn read_capped(reader: &mut (impl AsyncRead + Unpin), cap: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let n = match reader.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if buf.len() < cap {
            let room = cap - buf.len();
            buf.extend_from_slice(&tmp[..n.min(room)]);
        }
    }
    buf
}

fn extract_http_urls(text: &str) -> Vec<String> {
    url_regex()
        .find_iter(text)
        .map(|m| {
            m.as_str()
                .trim_end_matches(['.', ',', ';', ':', '!', '?', '\''])
                .to_string()
        })
        .collect()
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"https?://[^\s<>"'\]\)\}]+"#).expect("url pattern"))
}

/// A fetchable http(s) URL, or `None` when the target is local, private, or not http.
pub fn public_http_url(raw: &str) -> Option<String> {
    let raw = raw
        .trim()
        .trim_matches(|c: char| matches!(c, '<' | '>' | '"' | '\''));
    if raw.len() > 2_000 {
        return None;
    }
    let parsed = Url::parse(raw).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host_blocked(&host) {
        return None;
    }
    let mut clean = parsed;
    clean.set_fragment(None);
    Some(clean.to_string())
}

fn host_blocked(host: &str) -> bool {
    if host.is_empty() || host.contains('%') {
        return true;
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".arpa")
        || host == "metadata.google.internal"
    {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return !ip_is_public(ip);
    }
    if host.bytes().all(|b| b.is_ascii_digit()) {
        return match host.parse::<u32>() {
            Ok(n) => !ip_is_public(IpAddr::V4(Ipv4Addr::from(n))),
            Err(_) => true,
        };
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels
        .iter()
        .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()))
    {
        return true;
    }
    !host.contains('.')
}

fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_is_public(v4),
        IpAddr::V6(v6) => v6_is_public(v6),
    }
}

fn v4_is_public(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    if v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_unspecified()
        || o[0] == 0
        || o[0] >= 224
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
    {
        return false;
    }
    true
}

fn v6_is_public(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() || v6.is_unicast_link_local() {
        return false;
    }
    if let Some(v4) = v6.to_ipv4_mapped() {
        return v4_is_public(v4);
    }
    let o = v6.octets();
    if (o[0] & 0xfe) == 0xfc {
        return false;
    }
    if o[0] == 0x20 && o[1] == 0x01 && o[2] == 0x0d && o[3] == 0xb8 {
        return false;
    }
    true
}

async fn host_resolves_public(url: &str) -> bool {
    let Some(host) = Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
    else {
        return false;
    };
    tokio::task::spawn_blocking(move || host_resolves_public_blocking(&host))
        .await
        .unwrap_or(false)
}

fn host_resolves_public_blocking(host: &str) -> bool {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip_is_public(ip);
    }
    let Ok(addrs) = (host, 443).to_socket_addrs() else {
        return false;
    };
    let ips: Vec<SocketAddr> = addrs.collect();
    !ips.is_empty() && ips.iter().all(|addr| ip_is_public(addr.ip()))
}

fn page_key(url: &str) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        return url.trim().trim_end_matches('/').to_ascii_lowercase();
    };
    parsed.set_fragment(None);
    parsed.to_string().trim_end_matches('/').to_string()
}

fn html_to_text(html: &str, url: &str) -> (String, String) {
    let cleaned = strip_noise(html);
    let doc = scraper::Html::parse_document(&cleaned);
    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .map(|node| squash(&node.text().collect::<String>()))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| host_label(url));
    let text = tidy(&doc.root_element().text().collect::<Vec<_>>().join("\n"));
    (title, text)
}

fn strip_noise(html: &str) -> String {
    static RES: OnceLock<[Regex; 5]> = OnceLock::new();
    let res = RES.get_or_init(|| {
        ["script", "style", "noscript", "svg", "iframe"].map(|tag| {
            Regex::new(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}>")).expect("noise pattern")
        })
    });
    let mut out = html.to_string();
    for re in res {
        out = re.replace_all(&out, " ").into_owned();
    }
    out
}

fn title_from_text(text: &str, url: &str) -> String {
    for line in text.lines().take(40) {
        let line = line.trim().trim_start_matches('#').trim();
        let count = line.chars().count();
        if (3..=180).contains(&count) && !line.to_ascii_lowercase().starts_with("http") {
            return line.to_string();
        }
    }
    host_label(url)
}

fn host_label(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .host_str()
                .map(|host| host.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| url.to_string())
}

fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if !blank && !out.is_empty() {
                out.push('\n');
                blank = true;
            }
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        blank = false;
        out.push_str(&line);
    }
    out
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max_chars: usize) -> String {
    text.trim().chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(url: &str, content: Option<&str>) -> SearchResult {
        SearchResult {
            title: "T".into(),
            url: url.into(),
            snippet: "snippet".into(),
            content: content.map(str::to_string),
            ..SearchResult::default()
        }
    }

    #[test]
    fn links_in_a_question_are_kept_in_order() {
        let urls = user_urls(&[
            "see https://example.com/a, and https://example.com/a again. Also https://rust-lang.org/.",
        ]);
        assert_eq!(
            urls,
            vec![
                "https://example.com/a".to_string(),
                "https://rust-lang.org/".to_string()
            ]
        );
    }

    #[test]
    fn private_and_non_http_targets_are_refused() {
        assert!(public_http_url("https://example.com/a?q=1").is_some());
        for raw in [
            "http://127.0.0.1/latest",
            "http://169.254.169.254/latest/meta-data",
            "http://localhost/admin",
            "http://192.168.1.20/",
            "http://10.1.2.3/",
            "http://[::1]/",
            "http://2130706433/",
            "http://127.1/",
            "http://intranet/secret",
            "file:///etc/passwd",
            "https://user:pw@example.com/",
            "javascript:alert(1)",
        ] {
            assert!(public_http_url(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn user_links_come_before_snippets_and_long_pages_are_skipped() {
        let user = vec!["https://example.com/mine".into()];
        let results = vec![
            hit("https://example.com/already", Some(&"x".repeat(900))),
            hit("https://cdn.example/a.png", None),
            hit("https://example.com/one", None),
            hit("https://example.com/two", None),
        ];
        let plan = plan_reads(&user, &results, 2);
        assert_eq!(
            plan,
            vec![
                "https://example.com/mine".to_string(),
                "https://example.com/one".to_string(),
            ]
        );
    }

    #[test]
    fn a_fetched_user_page_is_inserted_ahead_of_search_hits() {
        let pages = vec![PageBody {
            url: "https://example.com/mine".into(),
            title: "Mine".into(),
            text: "The full article. ".repeat(40),
        }];
        let out = apply_pages(
            vec![hit("https://example.com/other", None)],
            &["https://example.com/mine".into()],
            &pages,
        );
        assert_eq!(out[0].url, "https://example.com/mine");
        assert_eq!(out[0].title, "Mine");
        assert!(out[0].content.as_ref().unwrap().contains("full article"));
        assert!(out[0].snippet.chars().count() <= SNIPPET_CHARS);
        assert_eq!(out[1].url, "https://example.com/other");
    }

    #[test]
    fn obscura_is_invoked_with_the_url_after_end_of_flags() {
        let args = obscura_args(true, true, 20, "https://example.com/a");
        assert_eq!(
            args,
            vec![
                "--stealth",
                "fetch",
                "--dump",
                "markdown",
                "--timeout",
                "20",
                "--quiet",
                "--",
                "https://example.com/a",
            ]
        );
    }

    #[test]
    fn a_summary_link_is_still_a_page_to_open() {
        let q = "https://www.hk01.com/%E6%95%B8%E7%A2%BC%E7%94%9F%E6%B4%BB/60398823/nvidia-rtx-5090 總結";
        assert_eq!(user_urls(&[q]).len(), 1);
    }

    #[test]
    fn the_article_html_is_kept_when_a_script_mentions_the_ad_wall() {
        let body = "受AI市場需求影響，NVIDIA 傳出 RTX 5090 全面停產，GB202 晶片轉供數據中心，旗艦遊戲卡面臨停產。".repeat(12);
        let html = format!(
            r#"<html><head><title>NVIDIA RTX 5090傳全面停產</title>
            <script>Please allow ads on this site. Integrity check failed.</script>
            </head><body><article><p>{body}</p></article></body></html>"#
        );
        let (title, text) = html_to_text(&html, "https://www.hk01.com/a");
        assert!(title.contains("5090"));
        assert!(text.contains("GB202"));
        assert!(is_document(&text));
        assert!(!usable(
            "# Please allow ads on this site\n\nOur site is supported by ads."
        ));
    }

    #[test]
    fn an_ad_wall_is_not_the_article() {
        let wall = r#"![](https://error-report.com/favicons/www.hk01.com)hk01.com

# Please allow ads on this site

Our site is supported by ads. Please consider allowing ads on this site so that we can continue to provide quality content.
"#;
        assert!(!usable(wall));
        assert!(usable(
            "NVIDIA RTX 5090傳全面停產。受AI市場龐大需求影響，GB202晶片產能轉供數據中心，旗艦遊戲卡面臨停產。"
        ));
    }

    #[test]
    fn html_drops_scripts_and_keeps_the_title() {
        let (title, text) = html_to_text(
            "<html><head><title>Hello</title><style>body{}</style></head><body><script>secret()</script><p>Visible fact</p></body></html>",
            "https://example.com/",
        );
        assert_eq!(title, "Hello");
        assert!(text.contains("Visible fact"));
        assert!(!text.contains("secret"));
    }
}
