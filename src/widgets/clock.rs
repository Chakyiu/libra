//! Clock skill. A place name ("Tokyo") or an IANA id ("Asia/Tokyo") becomes
//! the current time in that place, so the answer does not reuse the server clock.

use chrono::{Offset, TimeZone};
use chrono_tz::Tz;
use reqwest::Client;
use serde_json::{Value, json};

use crate::blocks::{Block, BlockType};

use super::{Here, WidgetRequest, wants_here};

struct Resolved {
    place: String,
    zone: String,
    machine: String,
    spoken: String,
    abbr: String,
}

/// The place a clock question names. Empty means the user's own clock.
/// `None` means the question is not asking for the time.
pub fn clock_place(query: &str) -> Option<String> {
    let raw = query.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_lowercase();
    const NOT_CLOCK: &[&str] = &[
        "first time",
        "every time",
        "one time",
        "last time",
        "next time",
        "this time",
        "same time",
        "by the time",
        "on time",
        "time to ",
        "time for ",
        "how many times",
        "how much time",
        "what time does",
        "what time did",
        "what time will",
        "what time should",
        "what time can",
        "what time do ",
    ];
    if NOT_CLOCK.iter().any(|phrase| lower.contains(phrase)) {
        return None;
    }

    // Longer leads first so "what time is it in " wins over "time in ".
    const LEADS: &[&str] = &[
        "what time is it in ",
        "what time is it at ",
        "what's the time in ",
        "what's the time at ",
        "whats the time in ",
        "whats the time at ",
        "what is the time in ",
        "what is the time at ",
        "current time in ",
        "current time at ",
        "the time in ",
        "the time at ",
        "timezone in ",
        "timezone of ",
        "timezone for ",
        "time zone in ",
        "time zone of ",
        "time zone for ",
        "time in ",
        "time at ",
    ];
    for lead in LEADS {
        if let Some(idx) = lower.find(lead) {
            if idx > 40 {
                continue;
            }
            let place = trim_place(&raw[idx + lead.len()..]);
            if reasonable_place(&place) {
                return Some(place);
            }
        }
    }

    let bare = trim_place(raw);
    let bare_l = bare.to_lowercase();
    for suffix in [" timezone", " time zone", " time"] {
        if let Some(city) = bare_l.strip_suffix(suffix) {
            let city = city.trim();
            if city.is_empty() || not_a_place(city) || city.split_whitespace().count() > 4 {
                continue;
            }
            let place = bare[..bare.len() - suffix.len()].trim();
            if !place.is_empty() {
                return Some(place.to_string());
            }
        }
    }

    const HERE: &[&str] = &[
        "what time is it",
        "what's the time",
        "whats the time",
        "what is the time",
        "current time",
        "time now",
        "local time",
        "the time",
        "time",
        "what's the date",
        "whats the date",
        "what is the date",
        "what day is it",
        "what's today's date",
        "whats today's date",
        "what is today's date",
        "today's date",
        "todays date",
    ];
    if HERE.contains(&bare_l.as_str()) {
        Some(String::new())
    } else {
        None
    }
}

/// When the question names a clock, make sure a time widget carries that place.
pub fn bind_clock(requested: &mut Vec<WidgetRequest>, query: &str) {
    let Some(place) = clock_place(query) else {
        return;
    };
    let time_at: Vec<usize> = requested
        .iter()
        .enumerate()
        .filter(|(_, w)| w.name.eq_ignore_ascii_case("time"))
        .map(|(i, _)| i)
        .collect();
    if time_at.is_empty() {
        requested.push(WidgetRequest {
            name: "time".into(),
            input: place,
        });
        return;
    }
    // One named place replaces a blank or a guessed zone. Two widgets were
    // already split by the model ("Tokyo" and "New York"), so leave them.
    if place.is_empty() || time_at.len() != 1 || place.to_lowercase().contains(" and ") {
        return;
    }
    requested[time_at[0]].input = place;
}

/// A question whose only job is the current time, so web search would only confuse it.
pub fn pure_clock(query: &str, requested: &[WidgetRequest]) -> bool {
    clock_place(query).is_some()
        && !requested.is_empty()
        && requested
            .iter()
            .all(|w| w.name.eq_ignore_ascii_case("time"))
}

/// Text the answer model must quote. The numbers are already in the right zone.
pub fn clock_facts(blocks: &[Block]) -> String {
    let mut lines = Vec::new();
    for block in blocks {
        let Some(data) = &block.data else {
            continue;
        };
        if data.get("name").and_then(Value::as_str) != Some("time") {
            continue;
        }
        let spoken = data.get("spoken").and_then(Value::as_str).unwrap_or("");
        if spoken.is_empty() {
            continue;
        }
        let place = data
            .get("place")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let zone = data
            .get("zone")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let where_ = if place.is_empty() { zone } else { place };
        if where_.is_empty() {
            continue;
        }
        if zone.is_empty() || zone == where_ || zone == "local" {
            lines.push(format!("{where_} is {spoken}."));
        } else {
            lines.push(format!("{where_} ({zone}) is {spoken}."));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "Clock (already converted — state these numbers exactly, in the user's language; do not add or subtract hours; do not replace them with the server date or a time from the references; do not cite a source for this clock):\n{}",
        lines.join("\n")
    )
}

pub async fn render(http: &Client, input: &str, here: Option<&Here>) -> Option<Block> {
    let resolved = if wants_here(input) {
        from_here(here)
    } else if let Some(found) = resolve_local(input) {
        found
    } else {
        let query = place_query(input);
        let query = if query.is_empty() {
            input.trim().to_string()
        } else {
            query
        };
        if query.is_empty() {
            return None;
        }
        let (place, tz) = lookup(http, &query).await?;
        stamp(&place, tz)
    };
    Some(to_block(&resolved))
}

fn resolve_local(input: &str) -> Option<Resolved> {
    for candidate in candidates(input) {
        if let Some(resolved) = resolve_one(&candidate, input) {
            return Some(resolved);
        }
    }
    None
}

fn resolve_one(candidate: &str, original: &str) -> Option<Resolved> {
    if looks_like_offset(candidate) {
        return from_offset(candidate);
    }
    if let Some(tz) = parse_zone(candidate).or_else(|| alias(&normalize(candidate))) {
        return Some(stamp(&place_name(original, tz), tz));
    }
    let zones = zones_named(&normalize(candidate));
    if zones.is_empty() || !same_clock(&zones) {
        return from_offset(candidate);
    }
    let tz = prefer(&zones);
    Some(stamp(&place_name(original, tz), tz))
}

fn from_here(here: Option<&Here>) -> Resolved {
    if let Some(h) = here {
        if let Ok(tz) = h.timezone.parse::<Tz>() {
            let place = if h.label.trim().is_empty() {
                city_label(tz)
            } else {
                h.label.trim().to_string()
            };
            return stamp(&place, tz);
        }
    }
    let local = chrono::Local::now();
    Resolved {
        place: "local".into(),
        zone: "local".into(),
        machine: local.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
        spoken: local.format("%A, %d %B %Y, %H:%M UTC%:z").to_string(),
        abbr: local.format("%Z").to_string(),
    }
}

fn stamp(place: &str, tz: Tz) -> Resolved {
    let local = chrono::Utc::now().with_timezone(&tz);
    Resolved {
        place: place.to_string(),
        zone: tz.name().to_string(),
        machine: local.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
        spoken: local.format("%A, %d %B %Y, %H:%M UTC%:z").to_string(),
        abbr: local.format("%Z").to_string(),
    }
}

fn to_block(clock: &Resolved) -> Block {
    Block {
        block_type: BlockType::Widget,
        message: format!("{}: {}", clock.place, clock.spoken),
        data: Some(json!({
            "name": "time",
            "timezone": clock.place,
            "zone": clock.zone,
            "place": clock.place,
            "time": clock.machine,
            "spoken": clock.spoken,
            "abbr": clock.abbr,
        })),
        extra: None,
    }
}

fn place_name(input: &str, tz: Tz) -> String {
    let tidy = place_query(input);
    let tidy = if tidy.is_empty() {
        input.trim().to_string()
    } else {
        tidy
    };
    if tidy.chars().any(|c| !c.is_ascii()) {
        let city = tidy.split(',').next().unwrap_or(tidy.as_str()).trim();
        if city.is_empty() {
            city_label(tz)
        } else {
            city.to_string()
        }
    } else {
        city_label(tz)
    }
}

fn city_label(tz: Tz) -> String {
    let name = tz.name();
    name.rsplit('/').next().unwrap_or(name).replace('_', " ")
}

/// Cities and abbreviations that do not have their own IANA id.
/// "CST" is left unset: it means US Central and China Standard Time.
fn alias(key: &str) -> Option<Tz> {
    let name = match key {
        "beijing" | "peking" | "北京" | "china" | "prc" | "上海" => "Asia/Shanghai",
        "osaka" | "kyoto" | "yokohama" | "nagoya" | "sapporo" | "hiroshima" | "fukuoka"
        | "kobe" | "大阪" | "京都" | "東京" | "东京" | "japan" | "jst" => "Asia/Tokyo",
        "mumbai" | "bombay" | "delhi" | "new delhi" | "kolkata" | "calcutta" | "chennai"
        | "bangalore" | "bengaluru" | "india" | "ist" => "Asia/Kolkata",
        "nyc" | "new york city" | "紐約" | "纽约" => "America/New_York",
        "la" | "sf" | "san francisco" | "洛杉磯" | "洛杉矶" => "America/Los_Angeles",
        "hk" | "香港" | "hkt" => "Asia/Hong_Kong",
        "uk" | "britain" | "england" | "scotland" | "wales" | "倫敦" | "伦敦" | "bst" => {
            "Europe/London"
        }
        "korea" | "south korea" | "首爾" | "首尔" | "kst" => "Asia/Seoul",
        "taiwan" | "台北" | "臺北" => "Asia/Taipei",
        "新加坡" | "sgt" => "Asia/Singapore",
        "巴黎" => "Europe/Paris",
        "悉尼" | "aest" | "aedt" => "Australia/Sydney",
        "est" | "edt" | "et" => "America/New_York",
        "pst" | "pdt" | "pt" => "America/Los_Angeles",
        "mst" | "mdt" | "mt" => "America/Denver",
        "cet" | "cest" => "Europe/Paris",
        "eet" | "eest" => "Europe/Helsinki",
        "msk" => "Europe/Moscow",
        "nzst" | "nzdt" => "Pacific/Auckland",
        "hst" => "Pacific/Honolulu",
        _ => return None,
    };
    name.parse().ok()
}

fn zones_named(key: &str) -> Vec<Tz> {
    if key.is_empty() {
        return Vec::new();
    }
    chrono_tz::TZ_VARIANTS
        .iter()
        .copied()
        .filter(|tz| {
            let name = tz.name();
            !name.starts_with("Etc/") && city_key(name) == key
        })
        .collect()
}

fn same_clock(zones: &[Tz]) -> bool {
    let now = chrono::Utc::now().naive_utc();
    let mut offsets = zones
        .iter()
        .map(|tz| tz.offset_from_utc_datetime(&now).fix());
    let Some(first) = offsets.next() else {
        return false;
    };
    offsets.all(|offset| offset == first)
}

fn prefer(zones: &[Tz]) -> Tz {
    *zones
        .iter()
        .max_by_key(|tz| zone_score(tz.name()))
        .expect("at least one zone")
}

fn zone_score(name: &str) -> i32 {
    let mut score = name.matches('/').count() as i32;
    if name.contains('/') {
        score += 4;
    }
    const LEGACY: &[&str] = &["US/", "Canada/", "Brazil/", "Chile/", "Mexico/"];
    if LEGACY.iter().any(|prefix| name.starts_with(prefix)) {
        score -= 6;
    }
    score
}

fn city_key(name: &str) -> String {
    name.rsplit('/')
        .next()
        .unwrap_or(name)
        .replace('_', " ")
        .to_lowercase()
}

fn parse_zone(raw: &str) -> Option<Tz> {
    let raw = raw.trim();
    if raw.is_empty() || looks_like_offset(raw) {
        return None;
    }
    if let Ok(tz) = raw.parse::<Tz>() {
        return Some(tz);
    }
    let underscored = raw.replace(' ', "_");
    if underscored != raw {
        if let Ok(tz) = underscored.parse::<Tz>() {
            return Some(tz);
        }
    }
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() == 2 {
        let guess = format!("{}/{}", parts[0], parts[1]);
        if let Ok(tz) = guess.parse::<Tz>() {
            return Some(tz);
        }
    }
    None
}

/// IANA's `Etc/GMT+9` means UTC−9. A user who writes GMT+9 means UTC+9.
fn looks_like_offset(input: &str) -> bool {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let upper = compact.to_ascii_uppercase();
    let rest = upper
        .strip_prefix("UTC")
        .or_else(|| upper.strip_prefix("GMT"))
        .unwrap_or(upper.as_str());
    rest.starts_with('+') || rest.starts_with('-')
}

fn from_offset(input: &str) -> Option<Resolved> {
    let offset = parse_offset(input)?;
    let local = chrono::Utc::now().with_timezone(&offset);
    let label = local.format("UTC%:z").to_string();
    Some(Resolved {
        place: label.clone(),
        zone: label,
        machine: local.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
        spoken: local.format("%A, %d %B %Y, %H:%M UTC%:z").to_string(),
        abbr: String::new(),
    })
}

fn parse_offset(input: &str) -> Option<chrono::FixedOffset> {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let upper = compact.to_ascii_uppercase();
    let rest = if let Some(rest) = upper.strip_prefix("UTC") {
        rest
    } else if let Some(rest) = upper.strip_prefix("GMT") {
        rest
    } else if upper.starts_with('+') || upper.starts_with('-') {
        upper.as_str()
    } else {
        return None;
    };
    let (sign, body) = if let Some(body) = rest.strip_prefix('+') {
        (1_i32, body)
    } else if let Some(body) = rest.strip_prefix('-') {
        (-1, body)
    } else {
        return None;
    };
    if body.is_empty() {
        return None;
    }
    let (hours, minutes) = if let Some((hours, minutes)) = body.split_once(':') {
        (hours.parse::<i32>().ok()?, minutes.parse::<i32>().ok()?)
    } else if body.len() == 4 && body.chars().all(|c| c.is_ascii_digit()) {
        (body[..2].parse().ok()?, body[2..].parse().ok()?)
    } else {
        (body.parse::<i32>().ok()?, 0)
    };
    if !(0..=14).contains(&hours) || !(0..60).contains(&minutes) {
        return None;
    }
    let secs = sign * (hours * 3600 + minutes * 60);
    if secs >= 0 {
        chrono::FixedOffset::east_opt(secs)
    } else {
        chrono::FixedOffset::west_opt(-secs)
    }
}

async fn lookup(http: &Client, place: &str) -> Option<(String, Tz)> {
    let geo: Value = http
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[
            ("name", place),
            ("count", "5"),
            ("language", "en"),
            ("format", "json"),
        ])
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    from_geocode(&geo)
}

fn from_geocode(geo: &Value) -> Option<(String, Tz)> {
    let results = geo.get("results")?.as_array()?;
    let hit = results
        .iter()
        .max_by_key(|row| row.get("population").and_then(Value::as_i64).unwrap_or(0))?;
    let zone = hit.get("timezone")?.as_str()?.trim();
    let tz: Tz = zone.parse().ok()?;
    let label = label_from(hit);
    if label.is_empty() {
        Some((city_label(tz), tz))
    } else {
        Some((label, tz))
    }
}

fn label_from(hit: &Value) -> String {
    let name = hit.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let admin = hit
        .get("admin1")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let country = hit
        .get("country")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return String::new();
    }
    let mut parts = vec![name];
    if !admin.is_empty()
        && !admin.eq_ignore_ascii_case(name)
        && !admin.eq_ignore_ascii_case(country)
    {
        parts.push(admin);
    }
    if !country.is_empty() && !country.eq_ignore_ascii_case(name) {
        parts.push(country);
    }
    parts.join(", ")
}

fn candidates(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |value: &str| {
        let value = value
            .trim()
            .trim_matches(|c: char| c == '?' || c == '!' || c == '.');
        let value = value.trim();
        if !value.is_empty()
            && !out
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(value))
        {
            out.push(value.to_string());
        }
    };
    push(input);
    let cleaned = place_query(input);
    push(&cleaned);
    if let Some((city, _)) = cleaned.split_once(',') {
        push(city);
    }
    out
}

fn place_query(input: &str) -> String {
    const DROP: &[&str] = &[
        "time",
        "timezone",
        "zone",
        "current",
        "now",
        "clock",
        "what",
        "whats",
        "what's",
        "is",
        "it",
        "the",
        "in",
        "at",
        "please",
        "today",
        "local",
        "date",
        "right",
        "a",
        "an",
        "for",
        "me",
        "tell",
        "show",
        "give",
        "currently",
        "oclock",
        "o'clock",
        "exactly",
    ];
    input
        .split_whitespace()
        .filter(|word| {
            let bare = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '-');
            !DROP.contains(&bare.to_lowercase().as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize(input: &str) -> String {
    input
        .trim()
        .chars()
        .filter_map(|c| {
            if c == '_' || c == '/' || c == '-' || c == ',' {
                Some(' ')
            } else if c.is_alphanumeric() || c.is_whitespace() || c == '+' {
                Some(c.to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn trim_place(input: &str) -> String {
    let mut text = input
        .trim()
        .trim_end_matches(['?', '!', '.', '。', '？', '！'])
        .trim();
    loop {
        let lower = text.to_lowercase();
        let suffix = [" right now", " please", " today", " currently", " now"]
            .into_iter()
            .find(|suffix| lower.ends_with(suffix));
        let Some(suffix) = suffix else {
            break;
        };
        text = text[..text.len() - suffix.len()]
            .trim()
            .trim_end_matches(',');
    }
    text.to_string()
}

fn reasonable_place(place: &str) -> bool {
    let words = place.split_whitespace().count();
    (1..=5).contains(&words) && place.chars().count() <= 48 && !not_a_place(place)
}

fn not_a_place(city: &str) -> bool {
    matches!(
        city,
        "lunch"
            | "dinner"
            | "bed"
            | "tea"
            | "real"
            | "this"
            | "that"
            | "some"
            | "free"
            | "full"
            | "part"
            | "run"
            | "compile"
            | "build"
            | "test"
            | "play"
            | "work"
            | "life"
            | "story"
            | "song"
            | "movie"
            | "show"
            | "game"
            | "flight"
            | "arrival"
            | "departure"
            | "closing"
            | "opening"
            | "start"
            | "end"
            | "due"
            | "screen"
            | "cpu"
            | "response"
            | "request"
            | "execution"
            | "runtime"
            | "good"
            | "bad"
            | "long"
            | "short"
            | "hard"
            | "spare"
            | "extra"
            | "day"
            | "night"
            | "summer"
            | "winter"
            | "past"
            | "future"
            | "same"
            | "one"
            | "no"
            | "more"
            | "a"
            | "the"
            | "my"
            | "your"
            | "our"
            | "their"
            | "its"
            | "local"
            | "standard"
            | "daylight"
            | "solar"
            | "mean"
            | "reaction"
            | "lead"
            | "cycle"
            | "half"
            | "down"
            | "idle"
            | "quiet"
            | "face"
            | "prime"
            | "air"
            | "drive"
            | "crunch"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(name: &str, input: &str) -> WidgetRequest {
        WidgetRequest {
            name: name.into(),
            input: input.into(),
        }
    }

    #[test]
    fn tokyo_is_japan_standard_time_not_the_server_clock() {
        let tokyo = resolve_local("Tokyo").unwrap();
        assert_eq!(tokyo.zone, "Asia/Tokyo");
        assert_eq!(tokyo.place, "Tokyo");
        assert!(tokyo.machine.contains("+09:00"), "{}", tokyo.machine);
        assert_eq!(tokyo.abbr, "JST");

        let hong_kong = resolve_local("Hong Kong").unwrap();
        assert_eq!(hong_kong.zone, "Asia/Hong_Kong");
        assert!(
            hong_kong.machine.contains("+08:00"),
            "{}",
            hong_kong.machine
        );
        assert_ne!(
            tokyo.machine.split(' ').nth(1),
            hong_kong.machine.split(' ').nth(1)
        );

        assert_eq!(resolve_local("tokyo time").unwrap().zone, "Asia/Tokyo");
        assert_eq!(resolve_local("Asia/Tokyo").unwrap().zone, "Asia/Tokyo");
        assert_eq!(resolve_local("Tokyo, Japan").unwrap().zone, "Asia/Tokyo");
        assert_eq!(resolve_local("JST").unwrap().zone, "Asia/Tokyo");
        assert_eq!(resolve_local("東京").unwrap().place, "東京");
        assert_eq!(resolve_local("東京").unwrap().zone, "Asia/Tokyo");
        assert_eq!(resolve_local("Osaka").unwrap().zone, "Asia/Tokyo");
        assert_eq!(resolve_local("Beijing").unwrap().zone, "Asia/Shanghai");
        assert_eq!(resolve_local("New York").unwrap().zone, "America/New_York");
        assert!(resolve_local("not a real place xyz").is_none());
    }

    #[test]
    fn gmt_plus_nine_means_ahead_of_utc() {
        let clock = resolve_local("GMT+9").unwrap();
        assert!(clock.machine.contains("+09:00"), "{}", clock.machine);
        assert!(!clock.machine.contains("-09:00"), "{}", clock.machine);
        let west = resolve_local("UTC-5").unwrap();
        assert!(west.machine.contains("-05:00"), "{}", west.machine);
    }

    #[test]
    fn a_named_place_is_pulled_out_of_the_question() {
        assert_eq!(
            clock_place("what time is it in Tokyo?"),
            Some("Tokyo".into())
        );
        assert_eq!(clock_place("tokyo time"), Some("tokyo".into()));
        assert_eq!(
            clock_place("what's the time in New York right now"),
            Some("New York".into())
        );
        assert_eq!(clock_place("what time is it?"), Some(String::new()));
        assert_eq!(clock_place("lunch time"), None);
        assert_eq!(clock_place("what time does the train leave"), None);
        assert_eq!(clock_place("weather in Paris"), None);

        let mut requested = vec![widget("time", "")];
        bind_clock(&mut requested, "what time is it in Tokyo?");
        assert_eq!(requested[0].input, "Tokyo");
        assert!(pure_clock("what time is it in Tokyo?", &requested));

        let mut guessed = vec![widget("time", "UTC")];
        bind_clock(&mut guessed, "tokyo time");
        assert_eq!(guessed[0].input, "tokyo");
    }

    #[test]
    fn the_user_clock_uses_the_browser_zone() {
        let here = Here {
            label: "Hong Kong".into(),
            latitude: f64::NAN,
            longitude: f64::NAN,
            timezone: "Asia/Hong_Kong".into(),
        };
        let clock = from_here(Some(&here));
        assert_eq!(clock.place, "Hong Kong");
        assert_eq!(clock.zone, "Asia/Hong_Kong");
        assert!(clock.machine.contains("+08:00"), "{}", clock.machine);
    }

    #[test]
    fn geocode_picks_the_largest_matching_place() {
        let geo = json!({
            "results": [
                {"name": "Springfield", "admin1": "Illinois", "country": "United States", "timezone": "America/Chicago", "population": 100000},
                {"name": "Springfield", "admin1": "Missouri", "country": "United States", "timezone": "America/Chicago", "population": 150000}
            ]
        });
        let (label, tz) = from_geocode(&geo).unwrap();
        assert_eq!(tz.name(), "America/Chicago");
        assert!(label.contains("Springfield"), "{label}");
        assert!(label.contains("Missouri"), "{label}");
    }

    #[test]
    fn clock_facts_tell_the_model_the_converted_time() {
        let clock = resolve_local("Tokyo").unwrap();
        let facts = clock_facts(&[to_block(&clock)]);
        assert!(facts.contains("Tokyo (Asia/Tokyo)"), "{facts}");
        assert!(facts.contains("+09:00"), "{facts}");
        assert!(facts.contains("do not add or subtract"), "{facts}");
    }
}
