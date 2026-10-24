use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::command;

const KEYRING_SERVICE: &str = "online.propnightrevival.launcher";
const KEYRING_USER: &str = "token";

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Settings {
    /// Token is kept in settings.json as the reliable store.
    /// Keyring is also written when available, but never required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub game_root: Option<String>,
    pub username: Option<String>,
    pub steam_id: Option<String>,
    #[serde(default)]
    pub hash_cache: HashMap<String, String>,
    #[serde(default)]
    pub dlc: HashMap<String, bool>,
}

fn settings_path() -> PathBuf {
    directories::ProjectDirs::from("online", "PropnightRevival", "Launcher")
        .map(|d| d.config_dir().join("settings.json"))
        .unwrap_or_else(|| PathBuf::from("settings.json"))
}

fn keyring_entry() -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| e.to_string())
}

/// Best-effort keyring write. Failures are ignored; the file is the source of truth.
fn try_keyring_save(token: Option<&str>) {
    if let Ok(entry) = keyring_entry() {
        match token {
            Some(t) if !t.is_empty() => { let _ = entry.set_password(t); }
            _ => { let _ = entry.delete_credential(); }
        }
    }
}

fn try_keyring_load() -> Option<String> {
    keyring_entry().ok().and_then(|e| e.get_password().ok()).filter(|t| !t.is_empty())
}

fn load_settings_sync() -> Result<Settings, String> {
    let path = settings_path();
    if !path.exists() {
        return Ok(Settings::default());
    }
    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

/// Single place every command should use to get the current token.
pub fn current_token() -> Option<String> {
    // 1. File (reliable)
    if let Ok(s) = load_settings_sync() {
        if let Some(t) = s.token.filter(|t| !t.is_empty()) {
            return Some(t);
        }
    }
    // 2. Keyring fallback
    try_keyring_load()
}

pub fn load_token() -> Option<String> {
    current_token()
}

pub fn save_token(token: Option<&str>) -> Result<(), String> {
    try_keyring_save(token);
    // Always persist in the settings file
    let mut s = load_settings_sync().unwrap_or_default();
    s.token = token.filter(|t| !t.is_empty()).map(String::from);
    save_settings_sync(s)
}

fn save_settings_sync(settings: Settings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let data = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
pub async fn load_settings() -> Result<Settings, String> {
    let mut s = load_settings_sync().unwrap_or_default();
    if s.token.is_none() {
        s.token = try_keyring_load();
    }
    Ok(s)
}

#[command]
pub async fn save_settings(mut settings: Settings) -> Result<(), String> {
    // Always keep the token in the file. Also try keyring.
    try_keyring_save(settings.token.as_deref());
    save_settings_sync(settings)
}
