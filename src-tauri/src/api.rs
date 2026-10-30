use crate::crypto::API_BASE;
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::command;

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub success: bool,
    pub token: Option<String>,
    pub username: Option<String>,
    pub steam_id: Option<String>,
    pub message: Option<String>,
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent("PropnightRevivalLauncher/linux")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

async fn send(
    client: &Client,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = format!("{API_BASE}{path}");
    let mut req = client.request(method, &url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let err = json
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("server error")
            .to_string();
        return Err(format!("{status}: {err}"));
    }
    Ok(json)
}

pub async fn login(username: String, password: String) -> Result<LoginResponse, String> {
    let c = client()?;
    let body = serde_json::json!({ "username": username, "password": password });
    let json = send(&c, Method::POST, "/revival/account/login", None, Some(body)).await?;
    parse_auth(json, username)
}

pub async fn register(username: String, password: String) -> Result<LoginResponse, String> {
    let c = client()?;
    let body = serde_json::json!({ "username": username, "password": password });
    let json = send(&c, Method::POST, "/revival/account/register", None, Some(body)).await?;
    parse_auth(json, username)
}

fn parse_auth(json: Value, username: String) -> Result<LoginResponse, String> {
    // token may be top-level or nested
    let token = json
        .get("token")
        .and_then(|v| v.as_str())
        .or_else(|| json.pointer("/account/token").and_then(|v| v.as_str()))
        .map(String::from);
    let steam_id = json
        .get("steamId")
        .and_then(|v| v.as_str())
        .or_else(|| json.pointer("/account/steamId").and_then(|v| v.as_str()))
        .map(String::from);
    let success = token.is_some();
    let message = if success {
        None
    } else {
        Some(
            json.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("auth failed (no token in response)")
                .to_string(),
        )
    };
    Ok(LoginResponse {
        success,
        token,
        username: Some(username),
        steam_id,
        message,
    })
}

pub async fn logout(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/account/logout", Some(token), Some(json!({}))).await
}

pub async fn me(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::GET, "/revival/account/me", Some(token), None).await
}

pub async fn sessions(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::GET, "/revival/sessions", Some(token), None).await
}

pub async fn servers(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::GET, "/revival/servers", Some(token), None).await
}

pub async fn friends(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::GET, "/revival/friends", Some(token), None).await
}

pub async fn friends_wait(token: &str, etag: &str) -> Result<Value, String> {
    let c = client()?;
    let body = json!({ "etag": etag });
    send(&c, Method::POST, "/revival/friends/wait", Some(token), Some(body)).await
}

pub async fn friend_request(token: &str, code: &str) -> Result<Value, String> {
    let c = client()?;
    let body = json!({ "code": code });
    send(&c, Method::POST, "/revival/friends/request", Some(token), Some(body)).await
}

pub async fn friend_action_raw(token: &str, action: &str, username: &str) -> Result<Value, String> {
    let c = client()?;
    let body = json!({ "username": username });
    send(&c, Method::POST, &format!("/revival/friends/{action}"), Some(token), Some(body)).await
}

pub async fn friends_post(token: &str, path: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, &format!("/revival/friends/{path}"), Some(token), Some(json!({}))).await
}

pub async fn squad(token: &str, action: &str, username: &str) -> Result<Value, String> {
    let c = client()?;
    let body = json!({ "username": username });
    send(&c, Method::POST, &format!("/revival/squad/{action}"), Some(token), Some(body)).await
}

pub async fn join_check(token: &str, port: u16, host: Option<&str>) -> Result<Value, String> {
    let c = client()?;
    let mut body = json!({ "port": port });
    if let Some(h) = host { body["host"] = json!(h); }
    send(&c, Method::POST, "/revival/join-check", Some(token), Some(body)).await
}

pub async fn host_public(token: &str, map: &str, role: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/host/public", Some(token), Some(json!({ "map": map, "role": role }))).await
}

pub async fn quickplay_choice(token: &str, id: &str, choice: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/quickplay/choice", Some(token), Some(json!({ "id": id, "choice": choice }))).await
}

pub async fn warning_seen(token: &str, id: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/warning/seen", Some(token), Some(json!({ "id": id }))).await
}

pub async fn redeem(token: &str, code: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/redeem", Some(token), Some(json!({ "code": code }))).await
}

pub async fn leave(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/leave", Some(token), Some(json!({}))).await
}

pub async fn patchnotes(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::GET, "/revival/patchnotes", Some(token), None).await
}

pub async fn set_username(token: &str, username: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/account/username", Some(token), Some(json!({ "username": username }))).await
}

pub async fn set_password(token: &str, password: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/account/password", Some(token), Some(json!({ "password": password }))).await
}

pub async fn set_email(token: &str, email: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/account/email", Some(token), Some(json!({ "email": email }))).await
}

pub async fn confirm_email(token: &str, code: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/account/email/confirm", Some(token), Some(json!({ "code": code }))).await
}

pub async fn delete_request(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/account/delete-request", Some(token), Some(json!({}))).await
}

pub async fn delete_cancel(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/account/delete-cancel", Some(token), Some(json!({}))).await
}

pub async fn max_progress(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/progress/max", Some(token), Some(json!({}))).await
}

pub async fn discord_link(token: &str) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/discord/link", Some(token), Some(json!({}))).await
}

pub async fn post_bug(token: &str, category: &str, description: &str, logs_b64: Option<&str>) -> Result<Value, String> {
    let c = client()?;
    let body = json!({
        "category": category,
        "description": description,
        "version": "0.1.0",
        "logs": logs_b64
    });
    send(&c, Method::POST, "/revival/bug", Some(token), Some(body)).await
}

pub async fn post_report(token: &str, report: Value) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/report", Some(token), Some(report)).await
}

pub async fn post_match(token: &str, match_data: Value) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::POST, "/revival/match", Some(token), Some(match_data)).await
}

#[command]
pub async fn get_me(token: String) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::GET, "/revival/account/me", Some(&token), None).await
}

#[command]
pub async fn get_profile(token: String) -> Result<Value, String> {
    let c = client()?;
    send(&c, Method::GET, "/revival/profile", Some(&token), None).await
}

#[command]
pub async fn post_profile(token: String, save_b64: String) -> Result<Value, String> {
    let c = client()?;
    let body = serde_json::json!({ "save": save_b64 });
    send(&c, Method::POST, "/revival/profile", Some(&token), Some(body)).await
}

#[command]
pub async fn presence(token: String, game: bool, role: String, server: String) -> Result<Value, String> {
    let c = client()?;
    let body = serde_json::json!({ "game": game, "role": role, "server": server });
    send(&c, Method::POST, "/revival/presence", Some(&token), Some(body)).await
}

#[command]
#[allow(dead_code)]
pub async fn friend_action(token: String, action: String, username: String) -> Result<Value, String> {
    let c = client()?;
    let body = serde_json::json!({ "username": username });
    send(
        &c,
        Method::POST,
        &format!("/revival/friends/{action}"),
        Some(&token),
        Some(body),
    )
    .await
}

/// Sync local SaveUserData.sav with the server (before launch).
#[command]
pub async fn sync_profile(token: String, game_root: String) -> Result<String, String> {
    let c = client()?;
    // On Linux the save is usually under the Proton prefix or ~/.local/share/Propnight
    let save_path = find_save_path_pub(&game_root)?;
    let local = if save_path.exists() {
        Some(std::fs::read(&save_path).map_err(|e| e.to_string())?)
    } else {
        None
    };

    let profile = send(&c, Method::GET, "/revival/profile", Some(&token), None).await?;
    if let Some(server_b64) = profile.get("save").and_then(|v| v.as_str()) {
        let server_bytes = STANDARD
            .decode(server_b64)
            .map_err(|e| format!("save decode: {e}"))?;
        let needs_write = match &local {
            Some(l) => l.as_slice() != server_bytes.as_slice(),
            None => true,
        };
        if needs_write {
            if let Some(parent) = save_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&save_path, &server_bytes).map_err(|e| e.to_string())?;
            return Ok("Wrote server profile to local save".into());
        }
        Ok("Local save already matches server".into())
    } else if local.is_some() {
        Ok("No server profile; keeping local save".into())
    } else {
        Ok("No local or server profile".into())
    }
}

pub fn find_save_path_pub(game_root: &str) -> Result<std::path::PathBuf, String> {
    // Prefer Proton prefix LocalAppData if present
    let _root = std::path::PathBuf::from(game_root);
    if let Some(steam) = crate::game::steam_candidates().into_iter().next() {
        let compat = steam.join("steamapps/compatdata/1549180/pfx/drive_c/users/steamuser/AppData/Local/Propnight/Saved/SaveGames/SaveUserData.sav");
        if compat.exists() || compat.parent().map(|p| p.exists()).unwrap_or(false) {
            return Ok(compat);
        }
    }
    // Fallback next to game or under XDG
    let fallback = dirs_home()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".local/share/Propnight/Saved/SaveGames/SaveUserData.sav");
    Ok(fallback)
}

fn dirs_home() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}
