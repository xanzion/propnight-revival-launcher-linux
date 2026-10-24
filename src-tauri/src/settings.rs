use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::command;

const KEYRING_SERVICE: &str = "online.propnightrevival.launcher";
const KEYRING_USER: &str = "token";

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Settings {
    /// Token is stored in the system keyring when available; this field is for fallback.
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

pub fn load_token() -> Option<String> {
    if let Ok(entry) = keyring_entry() {
        if let Ok(t) = entry.get_password() {
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    // fallback
    load_settings_sync().ok().and_then(|s| s.token)
}

pub fn save_token(token: Option<&str>) -> Result<(), String> {
    if let Ok(entry) = keyring_entry() {
        match token {
            Some(t) if !t.is_empty() => {
                let _ = entry.set_password(t);
            }
            _ => {
                let _ = entry.delete_credential();
            }
        }
    }
    Ok(())
}

fn load_settings_sync() -> Result<Settings, String> {
    let path = settings_path();
    if !path.exists() {
        return Ok(Settings::default());
    }
    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

#[command]
pub async fn load_settings() -> Result<Settings, String> {
    let mut s = load_settings_sync().unwrap_or_default();
    if s.token.is_none() {
        s.token = load_token();
    }
    Ok(s)
}

#[command]
pub async fn save_settings(mut settings: Settings) -> Result<(), String> {
    // Prefer keyring for the token
    if let Some(t) = settings.token.clone() {
        let _ = save_token(Some(&t));
        // Clear from file for security
        settings.token = None;
    } else {
        let _ = save_token(None);
    }

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
