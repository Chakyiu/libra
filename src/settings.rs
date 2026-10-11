use crate::config::{Config, SettingsHandle};
use crate::error::LibraError;
use serde_json::Value;
use std::path::Path;

/// Apply a partial settings patch, validate it, persist it, and swap the live config.
pub fn apply_patch(
    handle: &SettingsHandle,
    patch: &Value,
    db_path: &Path,
) -> Result<Value, LibraError> {
    let mut cfg = (*handle.get()).clone();
    cfg.apply_settings(patch);
    validate(&cfg, db_path)?;
    cfg.save_settings_file().map_err(LibraError::from)?;
    handle.set(cfg);
    Ok(handle.get().settings_json())
}

fn validate(cfg: &Config, db_path: &Path) -> Result<(), LibraError> {
    if !db_path.exists() {
        return Err(LibraError::BadRequest(format!(
            "database not reachable at {}; create a chat first",
            db_path.display()
        )));
    }
    let mut ids = std::collections::HashSet::new();
    for p in &cfg.providers {
        if p.id.trim().is_empty() || p.id.contains('/') {
            return Err(LibraError::BadRequest(format!(
                "provider '{}' needs an id without '/' — it prefixes its model names",
                p.name
            )));
        }
        if !ids.insert(p.id.as_str()) {
            return Err(LibraError::BadRequest(format!(
                "two providers share the id '{}'",
                p.id
            )));
        }
    }
    // Empty means not chosen yet: a fresh instance has no providers to pick from.
    for (label, reference) in [
        ("defaultModel", &cfg.default_model),
        ("defaultLightModel", &cfg.default_light_model),
        ("defaultRerankModel", &cfg.default_rerank_model),
    ] {
        if !reference.is_empty() && cfg.resolve_model(reference).is_none() {
            return Err(LibraError::BadRequest(format!(
                "{label} '{reference}' does not match a configured provider"
            )));
        }
    }
    if cfg.num_web_results == 0 || cfg.num_web_results > 50 {
        return Err(LibraError::BadRequest(
            "numWebResults must be between 1 and 50".into(),
        ));
    }
    if cfg.rerank_limit == 0 || cfg.rerank_limit > 20 {
        return Err(LibraError::BadRequest(
            "rerankLimit must be between 1 and 20".into(),
        ));
    }
    if !(0.0..=2.0).contains(&cfg.default_temperature) {
        return Err(LibraError::BadRequest(
            "defaultTemperature must be between 0 and 2".into(),
        ));
    }
    for kind in [
        crate::search::types::SearchKind::Web,
        crate::search::types::SearchKind::News,
        crate::search::types::SearchKind::Images,
        crate::search::types::SearchKind::Videos,
    ] {
        let order = cfg.search_order_for(kind);
        if order.is_empty() {
            return Err(LibraError::BadRequest(format!(
                "searchOrder for {} must list at least one provider",
                kind.as_str()
            )));
        }
    }
    Ok(())
}
