//! Answer widgets: calculator, time, weather, stocks. Ported from upstream `widgets/`.

mod calculator;
mod clock;
pub mod stocks;
pub mod weather;

use crate::blocks::{Block, BlockType};
use crate::config::SettingsHandle;
use calculator::eval;
use reqwest::Client;

pub use clock::{bind_clock, clock_facts, clock_place, pure_clock};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Deserialize)]
pub struct WidgetRequest {
    pub name: String,
    #[serde(default)]
    pub input: String,
}

/// A fix from the browser. Coordinates are NaN when only the timezone was available.
#[derive(Clone, Debug)]
pub struct Here {
    pub label: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
}

impl Here {
    pub fn coords(&self) -> Option<(f64, f64)> {
        if self.latitude.is_finite()
            && self.longitude.is_finite()
            && (-90.0..=90.0).contains(&self.latitude)
            && (-180.0..=180.0).contains(&self.longitude)
        {
            Some((self.latitude, self.longitude))
        } else {
            None
        }
    }
}

/// The question is about the user's own place, not a named one.
pub fn wants_here(input: &str) -> bool {
    let s = input
        .trim()
        .trim_end_matches(['?', '!', '.'])
        .trim()
        .to_lowercase();
    matches!(
        s.as_str(),
        "" | "here"
            | "near me"
            | "nearby"
            | "my location"
            | "current location"
            | "local"
            | "me"
            | "home"
    )
}

/// The question is about the user's own place even when the widget model forgets to say so.
pub fn query_needs_place(query: &str) -> bool {
    let q = query.to_lowercase();
    const HERE: &[&str] = &[
        "near me",
        "nearby",
        "around here",
        "around me",
        "in my area",
        "my city",
        "where i am",
        "my location",
        "current location",
    ];
    if HERE.iter().any(|phrase| q.contains(phrase)) {
        return true;
    }
    // A bare weather question. "weather in Paris" already names a place.
    const WEATHER: &[&str] = &[
        "what's the weather",
        "whats the weather",
        "what is the weather",
        "how's the weather",
        "hows the weather",
        "how is the weather",
        "weather today",
        "weather right now",
        "weather like",
        "current weather",
        "local weather",
        "the forecast",
        "weather forecast",
    ];
    WEATHER.iter().any(|phrase| q.contains(phrase))
        && !(q.contains(" in ") || q.contains(" at ") || q.contains(" for "))
}

/// `Some(true)` needs coordinates. `Some(false)` needs only the browser timezone.
pub fn location_request(
    requested: &[WidgetRequest],
    needs_location: bool,
    disabled: &[String],
) -> Option<bool> {
    let off = |name: &str| disabled.iter().any(|d| d.eq_ignore_ascii_case(name));
    let here = |name: &str| {
        !off(name)
            && requested
                .iter()
                .any(|w| w.name.eq_ignore_ascii_case(name) && wants_here(&w.input))
    };
    if needs_location || here("weather") {
        Some(true)
    } else if here("time") {
        Some(false)
    } else {
        None
    }
}

pub struct Registry {
    cfg: SettingsHandle,
    http: Client,
}

impl Registry {
    pub fn new(http: &Client, cfg: SettingsHandle) -> Self {
        Self {
            http: http.clone(),
            cfg,
        }
    }

    pub fn available(&self) -> Vec<&'static str> {
        vec!["calculator", "time", "weather", "stocks"]
    }

    pub async fn render(
        &self,
        requested: &[WidgetRequest],
        query: &str,
        disabled: &[String],
        here: Option<&Here>,
    ) -> Vec<Block> {
        let cfg = self.cfg.get();
        let mut blocks = Vec::new();
        for w in requested.iter().take(4) {
            let name = w.name.to_lowercase();
            if disabled.iter().any(|d| d.eq_ignore_ascii_case(&name))
                || !self.available().contains(&name.as_str())
            {
                continue;
            }
            let input = w.input.trim().to_string();
            let block = match name.as_str() {
                "calculator" => Self::calculator(&input, query),
                "time" => clock::render(&self.http, &input, here).await,
                "weather" => {
                    let coords = here.and_then(Here::coords);
                    if wants_here(&input) {
                        if let Some((lat, lon)) = coords {
                            let label = here.map(|h| h.label.as_str()).unwrap_or("");
                            weather::at(&self.http, &cfg, lat, lon, label).await
                        } else {
                            weather::current(&self.http, &cfg, &input, query).await
                        }
                    } else {
                        weather::current(&self.http, &cfg, &input, query).await
                    }
                }
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
    fn location_is_requested_only_when_the_place_is_the_users() {
        let none: &[String] = &[];
        assert_eq!(location_request(&[], false, none), None);
        assert_eq!(
            location_request(&[widget("weather", "Paris")], false, none),
            None
        );
        assert_eq!(
            location_request(&[widget("weather", "")], false, none),
            Some(true)
        );
        assert_eq!(
            location_request(&[widget("weather", "near me")], false, none),
            Some(true)
        );
        assert_eq!(location_request(&[], true, none), Some(true));
        assert_eq!(
            location_request(&[widget("time", "")], false, none),
            Some(false)
        );
        assert_eq!(
            location_request(&[widget("weather", "")], false, &["weather".into()]),
            None
        );
        assert!(wants_here("here?"));
        assert!(!wants_here("Tokyo"));
        assert!(query_needs_place("what's the weather"));
        assert!(query_needs_place("restaurants near me"));
        assert!(!query_needs_place("weather in Paris"));
        assert!(!query_needs_place("Hong Kong weather"));
        assert!(!query_needs_place("why is the sky blue"));
    }
}
