// libra desktop shell: a window whose only job is to open a libra server URL.
// It starts nothing, picks no port and owns no data directory. The start screen
// is the bundled page in ../ui; after a URL is opened the window belongs to that
// server's own page, and Tauri IPC is unavailable there (capabilities/default.json
// grants nothing to remote origins).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{fs, path::PathBuf};

use tauri::{AppHandle, Manager};
use url::Url;

/// The last URL the person opened. Kept next to the app's own config, never in
/// a libra data dir: this app has no libra data.
fn last_url_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("last-url.txt"))
}

fn read_last_url(app: &AppHandle) -> Option<String> {
    let raw = fs::read_to_string(last_url_path(app)?).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn save_last_url(app: &AppHandle, url: &str) {
    let Some(path) = last_url_path(app) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, url);
}

/// Accept what a person types: a full URL, or a bare `host:port` that gets
/// `http://`. Only http(s): a window onto a libra server, nothing else.
fn parse_server_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter the libra server URL.".into());
    }
    // A typed scheme (`http://`, `file://`) is taken literally and never gets a
    // prefix, so `http://` stays malformed instead of becoming host `http`.
    let url = if input.contains("://") {
        Url::parse(input).map_err(|_| "That does not look like a URL.".to_string())?
    } else {
        match Url::parse(input) {
            Ok(url) => url,
            Err(_) => Url::parse(&format!("http://{input}"))
                .map_err(|_| "That does not look like a URL.".to_string())?,
        }
    };
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "Only http and https URLs open here, not {scheme}:."
            ))
        }
    }
    if url.host_str().unwrap_or_default().is_empty() {
        return Err("That URL has no host.".into());
    }
    Ok(url)
}

/// Prefill for the start screen. Called by the bundled page only.
#[tauri::command]
fn last_server_url(app: AppHandle) -> Option<String> {
    read_last_url(&app)
}

/// Remember the URL, then let the server's own page take over the window.
#[tauri::command]
fn open_server_url(app: AppHandle, url: String) -> Result<(), String> {
    let target = parse_server_url(&url)?;
    save_last_url(&app, target.as_str());
    let window = app
        .get_webview_window("main")
        .ok_or("The window is gone.")?;
    window.navigate(target).map_err(|err| err.to_string())
}

fn main() {
    tauri::Builder::default()
        // Registered first so a second launch never builds a second window.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .invoke_handler(tauri::generate_handler![last_server_url, open_server_url])
        .run(tauri::generate_context!())
        .expect("libra desktop failed to start");
}

#[cfg(test)]
mod tests {
    use super::parse_server_url;

    #[test]
    fn accepts_http_and_https() {
        assert_eq!(
            parse_server_url("http://192.168.1.8:3001")
                .unwrap()
                .as_str(),
            "http://192.168.1.8:3001/"
        );
        assert_eq!(
            parse_server_url(" https://libra.example.com ")
                .unwrap()
                .as_str(),
            "https://libra.example.com/"
        );
    }

    #[test]
    fn bare_host_port_gets_http() {
        assert_eq!(
            parse_server_url("127.0.0.1:3001").unwrap().as_str(),
            "http://127.0.0.1:3001/"
        );
    }

    #[test]
    fn rejects_other_schemes_and_junk() {
        assert!(parse_server_url("file:///etc/passwd").is_err());
        assert!(parse_server_url("javascript:alert(1)").is_err());
        assert!(parse_server_url("   ").is_err());
        assert!(parse_server_url("http://").is_err());
    }
}
