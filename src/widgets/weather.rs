//! Current weather + 5-day forecast from Open-Meteo (keyless), like the original project.

use crate::blocks::{Block, BlockType};
use crate::config::Config;
use reqwest::Client;
use serde_json::{json, Value};

pub async fn current(http: &Client, cfg: &Config, location: &str, query: &str) -> Option<Block> {
    let place = if location.is_empty() { query } else { location };
    let geo: Value = http
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[("name", place), ("count", "1"), ("language", "en"), ("format", "json")])
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    let first = geo["results"].as_array()?.first()?;
    let lat = first["latitude"].as_f64()?;
    let lon = first["longitude"].as_f64()?;
    let name = format!("{}, {}", first["name"].as_str()?, first["country"].as_str().unwrap_or(""));

    let fc: Value = http
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude".to_string(), lat.to_string()),
            ("longitude".to_string(), lon.to_string()),
            ("current".into(), "temperature_2m,relative_humidity_2m,apparent_temperature,is_day,precipitation,weather_code,wind_speed_10m".into()),
            ("daily".into(), "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max".into()),
            ("timezone".into(), "auto".into()),
            ("forecast_days".into(), "5".into()),
        ])
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    let unit = if cfg.units_imperial() { "fahrenheit" } else { "celsius" };
    let cur = &fc["current"];
    let days = json!({
        "time": fc["daily"]["time"],
        "max": fc["daily"]["temperature_2m_max"],
        "min": fc["daily"]["temperature_2m_min"],
        "code": fc["daily"]["weather_code"],
        "precip": fc["daily"]["precipitation_probability_max"],
    });
    let temp = cur["temperature_2m"].as_f64().unwrap_or(f64::NAN);
    Some(Block {
        block_type: BlockType::Weather,
        message: format!("{name}: {}°{}", temp.round(), if unit == "fahrenheit" { "F" } else { "C" }),
        data: Some(json!({
            "location": name,
            "latitude": lat,
            "longitude": lon,
            "unit": unit,
            "current": {
                "temperature": temp,
                "feels_like": cur["apparent_temperature"],
                "humidity": cur["relative_humidity_2m"],
                "wind_speed": cur["wind_speed_10m"],
                "precipitation": cur["precipitation"],
                "code": cur["weather_code"],
                "is_day": cur["is_day"],
            },
            "forecast": days,
        })),
        extra: None,
    })
}
