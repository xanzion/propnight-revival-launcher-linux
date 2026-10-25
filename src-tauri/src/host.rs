//! Host command layer. Bridges the existing HTML UI to the Rust backend.
//! The UI calls `send(cmd, data)` which becomes `invoke(cmd, data)`.
//! Commands return objects that the existing `render()` function understands.

use crate::{api, game, settings, updater};
use base64::{engine::general_purpose::STANDARD, Engine};
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
    if s.token.is_none() { s.token = settings::current_token(); }
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
        start_background(app.clone()).await;
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
            let _ = settings::save_token(resp.token.as_deref());
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
            let _ = settings::save_token(resp.token.as_deref());
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
    if s.token.is_none() { s.token = settings::current_token(); }
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
    if s.token.is_none() { s.token = settings::current_token(); }
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
    if s.token.is_none() { s.token = settings::current_token(); }
    let Some(token) = s.token.filter(|t| !t.is_empty()) else {
        let state = json!({ "servers": [], "serversError": "not logged in" });
        emit_state(&app, state.clone());
        return Ok(state);
    };
    match api::servers(&token).await {
        Ok(resp) => {
            let list = resp.get("servers").cloned().unwrap_or(json!([]));
            let state = json!({ "servers": list });
            emit_state(&app, state.clone());
            Ok(state)
        }
        Err(e) => {
            let state = json!({ "servers": [], "serversError": e });
            emit_state(&app, state.clone());
            Ok(state)
        }
    }
}

/// One-shot friends long-poll. UI can call this in a loop.
#[command]
pub async fn friends_poll(app: AppHandle, etag: Option<String>) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let token = match s.token {
        Some(t) if !t.is_empty() => t,
        _ => return Err("not logged in (no token in keyring or settings)".into()),
    };
    let resp = api::friends_wait(&token, etag.as_deref().unwrap_or("")).await?;
    if let (Some(proof), Some(port), Some(relay)) = (
        resp.get("proof").and_then(|v| v.as_str()),
        resp.get("proofPort").and_then(|v| v.as_u64()),
        resp.pointer("/account/relayHost").and_then(|v| v.as_str()).or_else(|| resp.get("relayHost").and_then(|v| v.as_str())),
    ) {
        send_address_proof(relay, proof, port as u16);
    }
    let state = json!({
        "friends": resp,
        "etag": resp.get("etag")
    });
    emit_state(&app, state.clone());
    if let Some(inst) = game::detect_install().await.ok().flatten() {
        write_social_file(&inst.root, &resp);
    }
    // Flash window on new invite
    if resp.get("invites").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false) {
        if let Some(w) = window(&app) {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }
    Ok(state)
}

/// Post an in-game report.
#[command]
pub async fn report(app: AppHandle, report: Value) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let token = s.token.ok_or("not logged in")?;
    let _ = api::post_report(&token, report).await?;
    Ok(json!({ "toast": { "level": "good", "title": "Report sent", "text": "" } }))
}

/// Post a match result.
#[command]
pub async fn match_result(app: AppHandle, match_data: Value) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let token = s.token.ok_or("not logged in")?;
    let _ = api::post_match(&token, match_data).await?;
    Ok(json!({ "toast": { "level": "good", "title": "Match recorded", "text": "" } }))
}

/// List sessions (used internally; exposed for debugging).
#[command]
pub async fn list_sessions(_app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let token = s.token.ok_or("not logged in")?;
    api::sessions(&token).await
}

#[command]
pub async fn play(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let token = s.token.clone().ok_or("not logged in")?;
    let install = game::detect_install().await.map_err(|e| e.to_string())?;
    let Some(install) = install else {
        return Ok(json!({ "play": { "title": "Game not found", "sub": "Set path in Settings", "disabled": true } }));
    };
    let root = install.root.to_string_lossy().to_string();
    let win64 = install.root.join("Propnight/Binaries/Win64");

    // 1. Install / update revival files (required, same as original)
    emit_state(&app, json!({ "play": { "title": "UPDATING", "sub": "Checking revival files…", "busy": true, "disabled": true, "progress": -1 } }));
    let update = updater::check_updates(root.clone()).await;
    let update_msg = match &update {
        Ok(u) if !u.up_to_date => format!("Updated {} files", u.files_changed),
        Ok(_) => "Up to date".into(),
        Err(e) => {
            let state = json!({
                "play": { "title": "PLAY", "sub": format!("Update failed: {}", e), "disabled": false, "busy": false },
                "toast": { "level": "bad", "title": "Could not install revival files", "text": e }
            });
            emit_state(&app, state.clone());
            return Ok(state);
        }
    };

    // 2. Identity
    if let (Some(name), Some(sid)) = (&s.username, &s.steam_id) {
        let _ = game::set_identity(root.clone(), name.clone(), sid.clone()).await;
    }

    // 3. Profile sync before launch (best-effort)
    emit_state(&app, json!({ "play": { "title": "SYNCING", "sub": "Syncing your profile…", "busy": true, "disabled": true, "progress": -1 } }));
    let _ = api::sync_profile(token.clone(), root.clone()).await;

    // 4. Relay host + optional join command
    if let Ok(me) = api::me(&token).await {
        let relay = me.get("relayHost").and_then(|v| v.as_str())
            .or_else(|| me.pointer("/account/relayHost").and_then(|v| v.as_str()))
            .or_else(|| me.pointer("/account/relays/0/host").and_then(|v| v.as_str()));
        if let Some(host) = relay {
            let _ = std::fs::create_dir_all(&win64);
            let _ = std::fs::write(win64.join("revival-relay.txt"), host);
        }
    }
    // pending join is handled by the join command writing the file before calling play

    // 5. Launch and wait
    emit_state(&app, json!({ "play": { "title": "STARTING", "sub": "Launching via Steam…", "busy": true, "disabled": true, "progress": -1 } }));
    // Profile watch while game runs
    let token_watch = token.clone();
    let root_watch = root.clone();
    let watch = tokio::spawn(async move {
        let mut last_hash = String::new();
        loop {
            if let Ok(path) = crate::api::find_save_path_pub(&root_watch) {
                if path.exists() {
                    if let Ok(data) = std::fs::read(&path) {
                        let h = crate::crypto::sha256_bytes(&data);
                        if h != last_hash {
                            last_hash = h;
                            let _ = crate::api::sync_profile(token_watch.clone(), root_watch.clone()).await;
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    match game::launch_and_wait(&root).await {
        Ok(_) => {
            // 6. After exit: sync profile again
            emit_state(&app, json!({ "play": { "title": "SAVING", "sub": "Saving your profile…", "busy": true, "disabled": true, "progress": -1 } }));
            watch.abort();
            let _ = api::sync_profile(token, root).await;
            let state = json!({
                "play": { "title": "PLAY", "sub": "Everything is up to date.", "busy": false, "disabled": false },
                "splash": update_msg,
                "toast": { "level": "good", "title": "Profile saved", "text": "See you next game!" }
            });
            emit_state(&app, state.clone());
            Ok(state)
        }
        Err(e) => {
            let state = json!({
                "play": { "title": "PLAY", "sub": format!("Launch failed: {}", e), "disabled": false, "busy": false },
                "toast": { "level": "bad", "title": "Could not start", "text": e }
            });
            emit_state(&app, state.clone());
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
    // If a game process is likely running, ask the UI to confirm
    let install = game::detect_install().await.ok().flatten();
    let running = install.as_ref().map(|i| {
        std::process::Command::new("pgrep")
            .arg("-f")
            .arg("Propnight.exe")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }).unwrap_or(false);
    if running {
        emit_state(&app, json!({ "type": "confirmClose" }));
        return Ok(());
    }
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
    if s.token.is_none() { s.token = settings::current_token(); }
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
pub async fn join(app: AppHandle, port: Option<u16>, from: Option<String>, dlc: Option<bool>, map: Option<String>) -> Result<Value, String> {
    let port = port.unwrap_or(0);
    if !(7800..=7999).contains(&port) {
        return Ok(json!({ "toast": { "level": "warn", "title": "Join", "text": "Invalid port" } }));
    }

    // Busy guard
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if BUSY.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(json!({ "toast": { "level": "warn", "title": "Not now", "text": "Wait until the current task is finished." } }));
    }

    let install = game::detect_install().await.ok().flatten();
    let Some(install) = install else {
        return Ok(json!({ "toast": { "level": "warn", "title": "Join", "text": "Game not found" } }));
    };
    let win64 = install.root.join("Propnight/Binaries/Win64");
    let _ = std::fs::create_dir_all(&win64);

    // Classic maps check
    if dlc.unwrap_or(false) {
        let s = settings::load_settings().await.unwrap_or_default();
        if !s.dlc.get("classic-maps").copied().unwrap_or(false) {
            let _ = std::fs::write(win64.join("revival-map-command.txt"), format!("missingmaps {}", map.as_deref().unwrap_or("")));
            emit_state(&app, json!({ "tab": "dlc" }));
            return Ok(json!({ "toast": { "level": "warn", "title": "Classic maps needed", "text": format!("{} plays on a classic map. Install Classic maps in the DLC tab.", from.as_deref().unwrap_or("Your friend")) } }));
        }
    }

    // Address proof so private games accept us
    if let Some(token) = settings::current_token() {
        if let Ok(me) = api::me(&token).await {
            let proof = me.get("proof").and_then(|v| v.as_str())
                .or_else(|| me.pointer("/you/proof").and_then(|v| v.as_str()));
            let proof_port = me.get("proofPort").and_then(|v| v.as_u64())
                .or_else(|| me.pointer("/you/proofPort").and_then(|v| v.as_u64()));
            let relay = me.get("relayHost").and_then(|v| v.as_str())
                .or_else(|| me.pointer("/account/relayHost").and_then(|v| v.as_str()))
                .or_else(|| me.pointer("/account/relays/0/host").and_then(|v| v.as_str()));
            if let (Some(p), Some(pp), Some(r)) = (proof, proof_port, relay) {
                send_address_proof(r, p, pp as u16);
            }
        }
    }

    let cmd = format!("joinrelay {}", port);
    let _ = std::fs::write(win64.join("revival-map-command.txt"), &cmd);

    // Already running: just write the command and let the mod connect
    let running = std::process::Command::new("pgrep")
        .arg("-f")
        .arg("Propnight.exe")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if running {
        return Ok(json!({ "toast": { "level": "good", "title": format!("Joining {}", from.as_deref().unwrap_or("your friend")), "text": "Propnight is connecting to the game." } }));
    }

    // Otherwise launch
    BUSY.store(true, std::sync::atomic::Ordering::SeqCst);
    let state = json!({ "toast": { "level": "info", "title": format!("Joining {}", from.as_deref().unwrap_or("your friend")), "text": "Propnight starts, then joins the game." } });
    emit_state(&app, state.clone());
    let _ = play(app).await;
    BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
    Ok(state)
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
    if let Some(dir) = directories::BaseDirs::new().map(|b| b.config_dir().join("PropnightRevival")) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    }
    Ok(())
}

#[command]
pub async fn uninstall(app: AppHandle) -> Result<Value, String> {
    let install = game::detect_install().await.ok().flatten();
    let Some(install) = install else {
        return Ok(json!({ "toast": { "level": "info", "title": "Nothing to uninstall", "text": "No Propnight folder is set." } }));
    };
    let root = &install.root;
    let backups = root.join("revival-backups");
    if !backups.exists() {
        return Ok(json!({ "toast": { "level": "warn", "title": "Uninstall", "text": "No backups found. Use Steam Verify integrity if needed." } }));
    }
    // Restore every file from revival-backups
    let mut restored = 0;
    for entry in walkdir::WalkDir::new(&backups).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(&backups).unwrap();
            let dest = root.join(rel);
            if let Some(parent) = dest.parent() { let _ = std::fs::create_dir_all(parent); }
            if std::fs::copy(entry.path(), &dest).is_ok() { restored += 1; }
        }
    }
    let _ = std::fs::remove_dir_all(&backups);
    // Remove mod files
    let win64 = root.join("Propnight/Binaries/Win64");
    for name in ["dwmapi.dll", "revival-relay.txt", "revival-map-command.txt", "revival-presence.txt", "revival-social.txt", "revival-social-actions.txt"] {
        let _ = std::fs::remove_file(win64.join(name));
    }
    let _ = std::fs::remove_dir_all(win64.join("ue4ss"));
    let state = json!({ "toast": { "level": "good", "title": "Revival uninstalled", "text": format!("Restored {} file(s). Press PLAY to reinstall.", restored) } });
    emit_state(&app, state.clone());
    Ok(state)
}

#[command(rename = "updateLauncher")]
pub async fn update_launcher(_app: AppHandle) -> Result<Value, String> {
    // AppImage self-replace is not yet automated. Direct the user.
    Ok(json!({ "toast": { "level": "info", "title": "Launcher update", "text": "Download the latest AppImage from the releases page and replace this one." } }))
}

#[command]
pub async fn bug(app: AppHandle, category: String, text: String) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    if let Some(token) = s.token {
        let logs_b64 = if let Some(dir) = directories::BaseDirs::new().map(|b| b.config_dir().join("PropnightRevival")) {
            let log = dir.join("launcher.log");
            if log.exists() {
                std::fs::read(&log).ok().map(|b| STANDARD.encode(b))
            } else { None }
        } else { None };
        match api::post_bug(&token, &category, &text, logs_b64.as_deref()).await {
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
pub async fn discord_link(app: AppHandle) -> Result<Value, String> {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let token = s.token.ok_or("not logged in")?;
    match api::discord_link(&token).await {
        Ok(resp) => {
            let url = resp.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if url.starts_with("https://discord.com/") {
                let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
            }
            emit_state(&app, json!({ "toast": { "level": "info", "title": "Link Discord", "text": "Accept in your browser, then come back here." } }));
            // Poll for up to 5 minutes
            let before = api::me(&token).await.ok().and_then(|m| m.get("discord").and_then(|v| v.as_str()).map(String::from));
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if let Ok(me) = api::me(&token).await {
                    if let Some(now) = me.get("discord").and_then(|v| v.as_str()) {
                        if Some(now) != before.as_deref() {
                            emit_state(&app, json!({ "discord": now, "toast": { "level": "good", "title": "Discord linked", "text": format!("{now} has the Verified role") } }));
                            return Ok(json!({ "discord": now }));
                        }
                    }
                }
            }
            Ok(json!({ "toast": { "level": "warn", "title": "Discord", "text": "Timed out waiting for link" } }))
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

    let install = game::detect_install().await.ok().flatten();
    let Some(install) = install else {
        let _ = send_dlc_state(&app).await;
        return Ok(json!({ "toast": { "level": "warn", "title": "DLC", "text": "Game not found" } }));
    };
    let root = install.root.to_string_lossy().to_string();

    if on {
        emit_state(&app, json!({ "tab": "home", "play": { "title": "DOWNLOADING", "sub": format!("Installing {}", id), "busy": true, "disabled": true, "progress": -1 } }));
        match updater::install_dlc(&root, &id, &app).await {
            Ok(n) => {
                let _ = send_dlc_state(&app).await;
                emit_state(&app, json!({ "play": { "title": "PLAY", "sub": "DLC installed", "busy": false, "disabled": false } }));
                Ok(json!({ "toast": { "level": "good", "title": format!("{} installed", id), "text": format!("{} file(s)", n) } }))
            }
            Err(e) => {
                let _ = send_dlc_state(&app).await;
                emit_state(&app, json!({ "play": { "title": "PLAY", "sub": "DLC install failed", "busy": false, "disabled": false } }));
                Ok(json!({ "toast": { "level": "bad", "title": "DLC download failed", "text": e } }))
            }
        }
    } else {
        // Remove files tagged with this dlc
        if let Ok(client) = reqwest::Client::builder().user_agent("PropnightRevivalLauncher/linux").build() {
            if let Ok(m) = updater::load_manifest(&client).await {
                for f in &m.files {
                    if f.dlc.as_deref() == Some(&id) {
                        let path = install.root.join(&f.path);
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
        let _ = send_dlc_state(&app).await;
        Ok(json!({ "toast": { "level": "good", "title": format!("{} removed", id), "text": "You can install them again at any time." } }))
    }
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
        s.token = settings::current_token();
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
        s.token = settings::current_token();
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
        let root = inst.root.to_string_lossy().to_string();
        let _ = game::deploy_ue4ss_settings(&root);
        match updater::plan_updates(&root).await {
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

/// Read Win64/revival-presence.txt and POST presence.
async fn push_presence() {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let Some(token) = s.token else { return };
    let install = game::detect_install().await.ok().flatten();
    let (in_game, role, server) = if let Some(inst) = install {
        let path = inst.root.join("Propnight/Binaries/Win64/revival-presence.txt");
        if path.exists() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let mut role = "menu".to_string();
                let mut server = String::new();
                for line in text.lines() {
                    if let Some((k, v)) = line.split_once('=') {
                        match k.trim() {
                            "role" => role = v.trim().to_string(),
                            "server" => server = v.trim().to_string(),
                            _ => {}
                        }
                    }
                }
                (true, role, server)
            } else { (false, "menu".into(), String::new()) }
        } else { (false, "menu".into(), String::new()) }
    } else { (false, "menu".into(), String::new()) };
    let _ = api::presence(token, in_game, role, server).await;
}

/// Process Win64/revival-social-actions.txt written by the mod.
async fn process_social_actions() {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let Some(token) = s.token else { return };
    let install = game::detect_install().await.ok().flatten();
    let Some(inst) = install else { return };
    let path = inst.root.join("Propnight/Binaries/Win64/revival-social-actions.txt");
    if !path.exists() { return }
    let taken = path.with_extension("taken");
    let _ = std::fs::rename(&path, &taken);
    if let Ok(text) = std::fs::read_to_string(&taken) {
        for line in text.lines().take(10) {
            let mut parts = line.trim().splitn(2, ' ');
            let action = parts.next().unwrap_or("").to_lowercase();
            let username = parts.next().unwrap_or("");
            if ["invite", "accept", "decline", "leave", "kick"].contains(&action.as_str()) {
                let _ = api::squad(&token, &action, username).await;
            }
        }
    }
    let _ = std::fs::remove_file(&taken);
}

/// Send UDP address proof so private games accept this client.
fn send_address_proof(relay_host: &str, proof: &str, port: u16) {
    use std::net::UdpSocket;
    if proof.len() < 32 { return }
    let mut bytes = [0u8; 17];
    bytes[0] = b'P';
    for i in 0..16 {
        if let Ok(b) = u8::from_str_radix(&proof[i*2..i*2+2], 16) {
            bytes[i+1] = b;
        }
    }
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        let _ = sock.send_to(&bytes, format!("{}:{}", relay_host, port));
        let _ = sock.send_to(&bytes, format!("{}:{}", relay_host, port));
    }
}

/// Background maintenance: presence, social actions, periodic status.
pub async fn start_background(app: AppHandle) {
    tokio::spawn(async move {
        loop {
            push_presence().await;
            process_social_actions().await;
            flush_reports_and_matches().await;
            // Server status every ~30s, update check every ~60s
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let c = COUNTER.fetch_add(1, Ordering::Relaxed);
            if c % 6 == 0 {
                let _ = refresh_server_status(app.clone()).await;
            }
            if c % 12 == 0 {
                let _ = check_update_status(app.clone()).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

/// Watch revival-reports.jsonl and revival-matches.jsonl and post them.
async fn flush_reports_and_matches() {
    let mut s = settings::load_settings().await.unwrap_or_default();
    if s.token.is_none() { s.token = settings::current_token(); }
    let Some(token) = s.token else { return };
    let install = game::detect_install().await.ok().flatten();
    let Some(inst) = install else { return };
    let win64 = inst.root.join("Propnight/Binaries/Win64");

    for (file, kind) in [("revival-reports.jsonl", "report"), ("revival-matches.jsonl", "match")] {
        let path = win64.join(file);
        if !path.exists() { continue; }
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut remaining = Vec::new();
            for line in text.lines() {
                if line.trim().is_empty() { continue; }
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    let res = if kind == "report" {
                        api::post_report(&token, v).await
                    } else {
                        api::post_match(&token, v).await
                    };
                    if res.is_err() {
                        remaining.push(line.to_string());
                    }
                }
            }
            if remaining.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else {
                let _ = std::fs::write(&path, remaining.join("\n") + "\n");
            }
        }
    }
}

/// Write revival-social.txt for the in-game friends/squad widget.
fn write_social_file(root: &std::path::Path, friends: &Value) {
    let win64 = root.join("Propnight/Binaries/Win64");
    let _ = std::fs::create_dir_all(&win64);
    let mut sb = String::new();
    if let Some(you) = friends.get("you").and_then(|v| v.get("username")).and_then(|v| v.as_str()) {
        sb.push_str(&format!("you\t{}\n", clean(you)));
    }
    if let Some(list) = friends.get("friends").and_then(|v| v.as_array()) {
        for fr in list {
            let name = fr.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let status = fr.get("status").and_then(|v| v.as_str()).unwrap_or("offline");
            let online = matches!(status, "playing" | "hosting" | "in_game");
            sb.push_str(&format!("friend\t{}\t{}\t{}\n", clean(name), if online { 1 } else { 0 }, clean(status)));
        }
    }
    if let Some(squad) = friends.get("squad") {
        if let Some(members) = squad.get("members").and_then(|v| v.as_array()) {
            for m in members {
                let name = m.get("username").and_then(|v| v.as_str()).unwrap_or("");
                let leader = m.get("leader").and_then(|v| v.as_bool()).unwrap_or(false);
                let online = m.get("online").and_then(|v| v.as_bool()).unwrap_or(false);
                sb.push_str(&format!("squad\t{}\t{}\t{}\n", clean(name), if leader { 1 } else { 0 }, if online { 1 } else { 0 }));
            }
        }
    }
    let _ = std::fs::write(win64.join("revival-social.txt"), sb);
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect()
}

/// Resize the window from an edge/corner.
/// Tauri 2.12 does not expose start_resize_dragging on WebviewWindow; drag-to-resize
/// is not available in this version. The grips remain for future support.
#[command]
pub async fn resize(_app: AppHandle, _edge: String) -> Result<(), String> {
    Ok(())
}
