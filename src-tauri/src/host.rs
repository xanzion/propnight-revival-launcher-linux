//! Host command layer. Bridges the existing HTML UI to the Rust backend.
//! The UI calls `send(cmd, data)` which becomes `invoke(cmd, data)`.
//! Commands return objects that the existing `render()` function understands.

use crate::{api, game, settings, updater};
use serde_json::{json, Value};
use tauri::{command, AppHandle, Emitter, Manager, WebviewWindow};

fn emit_state(app: &AppHandle, state: Value) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.emit("state", &state);
    }
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

#[command]
pub async fn ready(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let install = game::detect_install().await.ok().flatten();
    let view = if s.token.is_some() { "home" } else { "auth" };
    let state = json!({
        "view": view,
        "username": s.username,
        "steamId": s.steam_id,
        "gameRoot": install.as_ref().map(|i| i.root.to_string_lossy()),
        "launcherVersion": "0.1.0",
        "splash": "Ready"
    });
    emit_state(&app, state.clone());
    if s.token.is_some() {
        let _ = refresh_server_status(app.clone()).await;
        let _ = check_update_status(app.clone()).await;
        let _ = send_dlc_state(&app).await;
    }
    Ok(state)
}

#[command]
pub async fn login(app: AppHandle, username: String, password: String) -> Result<Value, String> {
    match api::login(username.clone(), password).await {
        Ok(resp) if resp.success => {
            let mut s = settings::load_settings().await.unwrap_or_default();
            s.token = resp.token.clone();
            s.username = Some(username.clone());
            s.steam_id = resp.steam_id.clone();
            let _ = settings::save_settings(s).await;
            let state = json!({
                "view": "home",
                "username": username,
                "steamId": resp.steam_id,
                "authError": null
            });
            emit_state(&app, state.clone());
            let _ = refresh_me(app.clone()).await;
            let _ = refresh_server_status(app.clone()).await;
            let _ = check_update_status(app.clone()).await;
            let _ = send_dlc_state(&app).await;
            Ok(json!({
                "view": "home",
                "username": username,
                "steamId": resp.steam_id,
                "authError": null,
                "toast": { "level": "good", "title": "Signed in", "text": format!("Welcome, {username}") }
            }))
        }
        Ok(resp) => {
            let msg = resp.message.unwrap_or_else(|| "Login failed".into());
            let state = json!({
                "authError": msg,
                "toast": { "level": "bad", "title": "Sign in failed", "text": msg }
            });
            emit_state(&app, state.clone());
            Ok(state)
        }
        Err(e) => {
            let state = json!({
                "authError": e,
                "toast": { "level": "bad", "title": "Sign in failed", "text": e }
            });
            emit_state(&app, state.clone());
            Ok(state)
        }
    }
}

#[command]
pub async fn register(app: AppHandle, username: String, password: String) -> Result<Value, String> {
    match api::register(username.clone(), password).await {
        Ok(resp) if resp.success => {
            let mut s = settings::load_settings().await.unwrap_or_default();
            s.token = resp.token.clone();
            s.username = Some(username.clone());
            s.steam_id = resp.steam_id.clone();
            let _ = settings::save_settings(s).await;
            let state = json!({
                "view": "home",
                "username": username,
                "steamId": resp.steam_id,
                "authError": null,
                "toast": { "level": "good", "title": "Account created", "text": format!("Welcome, {username}") }
            });
            emit_state(&app, state.clone());
            let _ = refresh_me(app.clone()).await;
            let _ = refresh_server_status(app.clone()).await;
            let _ = check_update_status(app.clone()).await;
            let _ = send_dlc_state(&app).await;
            Ok(state)
        }
        Ok(resp) => {
            let msg = resp.message.unwrap_or_else(|| "Registration failed".into());
            let state = json!({
                "authError": msg,
                "toast": { "level": "bad", "title": "Account not created", "text": msg }
            });
            emit_state(&app, state.clone());
            Ok(state)
        }
        Err(e) => {
            let state = json!({
                "authError": e,
                "toast": { "level": "bad", "title": "Account not created", "text": e }
            });
            emit_state(&app, state.clone());
            Ok(state)
        }
    }
}

#[command]
pub async fn logout(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    if let Some(token) = &s.token {
        let _ = api::logout(token).await;
    }
    let mut cleared = s;
    cleared.token = None;
    cleared.username = None;
    let _ = settings::save_settings(cleared).await;
    let state = json!({ "view": "auth", "username": null, "steamId": null });
    emit_state(&app, state.clone());
    Ok(state)
}

/// Refresh account info (me) and push it to the UI.
#[command]
pub async fn refresh_me(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    let me = api::me(&token).await?;
    let state = json!({
        "username": me.get("username").and_then(|v| v.as_str()).or(s.username.as_deref()),
        "steamId": me.get("steamId").and_then(|v| v.as_str()).or(s.steam_id.as_deref()),
        "discord": me.get("discord")
    });
    emit_state(&app, state.clone());
    Ok(state)
}

/// Fetch public games for the servers tab.
#[command]
pub async fn refresh_servers(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    let servers = api::servers(&token).await?;
    let state = json!({ "servers": servers.get("servers").cloned().unwrap_or(json!([])) });
    emit_state(&app, state.clone());
    Ok(state)
}

/// One-shot friends long-poll. UI can call this in a loop.
#[command]
pub async fn friends_poll(app: AppHandle, etag: Option<String>) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    let resp = api::friends_wait(&token, etag.as_deref().unwrap_or("")).await?;
    let state = json!({
        "friends": resp,
        "etag": resp.get("etag")
    });
    emit_state(&app, state.clone());
    Ok(state)
}

/// Post an in-game report.
#[command]
pub async fn report(app: AppHandle, report: Value) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    let _ = api::post_report(&token, report).await?;
    Ok(json!({ "toast": { "level": "good", "title": "Report sent", "text": "" } }))
}

/// Post a match result.
#[command]
pub async fn match_result(app: AppHandle, match_data: Value) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    let _ = api::post_match(&token, match_data).await?;
    Ok(json!({ "toast": { "level": "good", "title": "Match recorded", "text": "" } }))
}

/// List sessions (used internally; exposed for debugging).
#[command]
pub async fn list_sessions(_app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    api::sessions(&token).await
}

#[command]
pub async fn play(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let install = game::detect_install().await.map_err(|e| e.to_string())?;
    let Some(install) = install else {
        return Ok(json!({ "play": { "title": "Game not found", "sub": "Set path in Settings", "disabled": true } }));
    };

    // Check / apply updates
    let update = updater::check_updates(install.root.to_string_lossy().to_string()).await;
    let msg = match &update {
        Ok(u) if !u.up_to_date => format!("Updated {} files", u.files_changed),
        Ok(_) => "Up to date".into(),
        Err(e) => format!("Update skipped: {e}"),
    };

    // Identity
    if let (Some(name), Some(sid)) = (&s.username, &s.steam_id) {
        let _ = game::set_identity(
            install.root.to_string_lossy().to_string(),
            name.clone(),
            sid.clone(),
        )
        .await;
    }

    // Launch
    match game::launch_game(install.root.to_string_lossy().to_string()).await {
        Ok(info) => {
            let state = json!({
                "play": { "title": "Running", "sub": info, "busy": true },
                "splash": msg
            });
            emit_state(&app, state.clone());
            let _ = refresh_me(app.clone()).await;
            Ok(state)
        }
        Err(e) => {
            let state = json!({
                "play": { "title": "Launch failed", "sub": e, "disabled": false }
            });
            emit_state(&app, state.clone());
            let _ = refresh_me(app.clone()).await;
            Ok(state)
        }
    }
}

#[command]
pub async fn minimize(app: AppHandle) -> Result<(), String> {
    if let Some(w) = window(&app) {
        w.minimize().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[command]
pub async fn maximize(app: AppHandle) -> Result<(), String> {
    if let Some(w) = window(&app) {
        if w.is_maximized().unwrap_or(false) {
            w.unmaximize().map_err(|e| e.to_string())?;
        } else {
            w.maximize().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[command]
pub async fn close(app: AppHandle) -> Result<(), String> {
    if let Some(w) = window(&app) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[command(rename = "closeAnyway")]
pub async fn close_anyway(app: AppHandle) -> Result<(), String> {
    close(app).await
}

#[command]
pub async fn drag(app: AppHandle) -> Result<(), String> {
    if let Some(w) = window(&app) {
        let _ = w.start_dragging();
    }
    Ok(())
}

#[command]
pub async fn friend(app: AppHandle, action: String, username: Option<String>, code: Option<String>) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    let result = match action.as_str() {
        "request" => {
            if let Some(c) = code.or(username) {
                api::friend_request(&token, &c).await
            } else {
                Err("no code".into())
            }
        }
        "declineAll" => api::friends_post(&token, "decline-all").await,
        "copyCode" => {
            let friends = api::friends(&token).await?;
            let code = friends.get("code").and_then(|v| v.as_str()).unwrap_or("");
            return Ok(json!({ "toast": { "level": "good", "title": "Friend code copied", "text": code } }));
        }
        a if a.starts_with("squad") => {
            let act = &a[5..];
            api::squad(&token, act, username.as_deref().unwrap_or("")).await
        }
        "accept" | "decline" | "remove" | "invite" => {
            if let Some(u) = username {
                api::friend_action_raw(&token, &action, &u).await
            } else {
                Err("no username".into())
            }
        }
        _ => Err(format!("unknown friend action: {action}")),
    };
    let friends = api::friends(&token).await.unwrap_or(json!({ "friends": [] }));
    let state = json!({
        "friends": friends,
        "toast": result.as_ref().ok().map(|_| json!({ "level": "good", "title": "Done", "text": &action }))
    });
    emit_state(&app, state.clone());
    Ok(state)
}

#[command]
pub async fn join(_app: AppHandle, port: Option<u16>, from: Option<String>) -> Result<Value, String> {
    // Joining is handled by the game/mod; launcher just records intent for now.
    Ok(json!({ "toast": { "level": "info", "title": "Join", "text": format!("Joining {} on port {}", from.unwrap_or_default(), port.unwrap_or(0)) } }))
}

#[command(rename = "serversTab")]
pub async fn servers_tab(app: AppHandle, on: bool) -> Result<(), String> {
    if on {
        let _ = refresh_servers(app).await;
    }
    Ok(())
}

#[command(rename = "pageError")]
pub async fn page_error(message: String) -> Result<(), String> {
    tracing::warn!("page error: {message}");
    Ok(())
}

#[command]
pub async fn pick_folder(app: AppHandle) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;
    let folder = app.dialog().file().blocking_pick_folder();
    if let Some(path) = folder {
        let p = path.to_string();
        let mut s = settings::load_settings().await.unwrap_or_default();
        s.game_root = Some(p.clone());
        let _ = settings::save_settings(s).await;
        let state = json!({ "gameRoot": p, "toast": { "level": "good", "title": "Game path", "text": p } });
        emit_state(&app, state.clone());
        Ok(state)
    } else {
        Ok(json!({ "toast": { "level": "info", "title": "Cancelled", "text": "No folder selected" } }))
    }
}

#[command]
pub async fn open_data(_app: AppHandle) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn uninstall(_app: AppHandle) -> Result<Value, String> {
    Ok(json!({ "toast": { "level": "info", "title": "Uninstall", "text": "Not yet implemented" } }))
}

#[command(rename = "updateLauncher")]
pub async fn update_launcher(_app: AppHandle) -> Result<Value, String> {
    Ok(json!({ "toast": { "level": "info", "title": "Launcher", "text": "No launcher self-update yet" } }))
}

#[command]
pub async fn bug(app: AppHandle, category: String, text: String) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    if let Some(token) = s.token {
        match api::post_bug(&token, &category, &text, None).await {
            Ok(_) => {
                let state = json!({ "type": "bugSent", "ok": true, "toast": { "level": "good", "title": "Bug report sent", "text": "Thanks!" } });
                emit_state(&app, state.clone());
                return Ok(state);
            }
            Err(e) => {
                let state = json!({ "type": "bugSent", "ok": false, "toast": { "level": "bad", "title": "Report not sent", "text": e } });
                emit_state(&app, state.clone());
                return Ok(state);
            }
        }
    }
    Ok(json!({ "type": "bugSent", "ok": false }))
}

#[command(rename = "discordLink")]
pub async fn discord_link(_app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::load_token(); }
    let token = s.token.ok_or("not logged in")?;
    match api::discord_link(&token).await {
        Ok(resp) => {
            let url = resp.get("url").and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!({ "toast": { "level": "info", "title": "Discord", "text": format!("Open: {url}") } }))
        }
        Err(e) => Ok(json!({ "toast": { "level": "bad", "title": "Discord", "text": e } })),
    }
}

#[command(rename = "dlcRefresh")]
pub async fn dlc_refresh(app: AppHandle) -> Result<(), String> {
    send_dlc_state(&app).await
}

#[command]
pub async fn dlc(app: AppHandle, id: String, on: bool) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    s.dlc.insert(id.clone(), on);
    let _ = settings::save_settings(s.clone()).await;
    // Re-emit DLC list so the toggle reflects immediately
    let _ = send_dlc_state(&app).await;
    Ok(json!({
        "toast": { "level": "good", "title": "DLC", "text": format!("{} {}", id, if on { "enabled" } else { "disabled" }) }
    }))
}

/// Load the signed manifest and emit the DLC packs the UI expects.
async fn send_dlc_state(app: &AppHandle) -> Result<(), String> {
    let s = settings::load_settings().await.unwrap_or_default();
    let client = reqwest::Client::builder()
        .user_agent("PropnightRevivalLauncher/linux")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let data = client
        .get(format!("{}?t={}", crate::crypto::MANIFEST_URL, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)))
        .send().await.map_err(|e| e.to_string())?
        .bytes().await.map_err(|e| e.to_string())?;
    let manifest: serde_json::Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    let files_url = manifest.get("filesUrl").and_then(|v| v.as_str()).unwrap_or("");
    let packs = manifest.get("dlc").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for p in packs {
        let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let maps = p.get("maps").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut maps_out = Vec::new();
        for m in maps {
            let img_sha = m.get("image").and_then(|i| i.get("sha256")).and_then(|v| v.as_str()).unwrap_or("");
            maps_out.push(json!({
                "name": m.get("name"),
                "from": m.get("from"),
                "image": format!("{}{}", files_url, img_sha)
            }));
        }
        out.push(json!({
            "id": id,
            "title": p.get("title"),
            "description": p.get("description"),
            "size": p.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
            "on": s.dlc.get(&id).copied().unwrap_or(false),
            "maps": maps_out
        }));
    }
    let state = json!({ "dlc": out });
    emit_state(app, state);
    Ok(())
}

/// Refresh the "Connecting…" chip by querying /revival/sessions.
#[command]
pub async fn refresh_server_status(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() {
        s.token = settings::load_token();
    }
    let token = s.token.as_deref();
    let mut server = json!({ "online": false, "games": 0, "players": 0 });
    if let Some(t) = token {
        match api::sessions(t).await {
            Ok(resp) => {
                let games = resp.get("sessions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let players: i64 = games.iter().filter_map(|g| g.get("players").and_then(|p| p.as_i64())).sum();
                server = json!({
                    "online": true,
                    "games": games.len(),
                    "players": players
                });
            }
            Err(_) => {
                // leave offline
            }
        }
    }
    let state = json!({ "server": server });
    emit_state(&app, state.clone());
    Ok(state)
}

/// Quiet update check that fills the "Checking for updates…" chip.
#[command]
pub async fn check_update_status(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() {
        s.token = settings::load_token();
    }
    let install = game::detect_install().await.ok().flatten();
    let mut update = json!({
        "checking": false,
        "error": false,
        "launcher": null,
        "version": null,
        "gameBytes": 0
    });
    if let Some(inst) = install {
        match updater::plan_updates(&inst.root.to_string_lossy()).await {
            Ok((version, count)) => {
                update = json!({
                    "checking": false,
                    "error": false,
                    "launcher": null,
                    "version": version,
                    "gameBytes": count
                });
            }
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                update["error"] = json!(true);
            }
        }
    } else {
        // no game found – still mark as checked
        update["version"] = json!("no-game");
    }
    let state = json!({ "update": update });
    emit_state(&app, state.clone());
    Ok(state)
}
