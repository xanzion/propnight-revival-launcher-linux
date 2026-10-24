use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{command, Emitter, Manager};

const EXE_RELATIVE: &str = "Propnight/Binaries/Win64/Propnight.exe";

#[derive(Debug, Serialize, Deserialize)]
pub struct GameInstall {
    pub root: PathBuf,
    pub exe: PathBuf,
    pub valid: bool,
}

fn is_game_root(dir: &Path) -> bool {
    dir.join(EXE_RELATIVE).exists()
}

pub fn steam_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = home_dir() {
        for p in [
            home.join(".local/share/Steam"),
            home.join(".steam/steam"),
            home.join(".steam/root"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"), // Flatpak
        ] {
            if p.exists() {
                out.push(p);
            }
        }
    }
    // Also check XDG
    if let Some(data) = data_dir() {
        let p = data.join("Steam");
        if p.exists() {
            out.push(p);
        }
    }
    out
}

fn parse_libraryfolders(vdf: &Path) -> Vec<PathBuf> {
    let mut libs = Vec::new();
    if let Ok(text) = std::fs::read_to_string(vdf) {
        // Simple regex-like scan for "path" "..."
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("\"path\"") {
                if let Some(start) = rest.find('"') {
                    if let Some(end) = rest[start + 1..].find('"') {
                        let path = &rest[start + 1..start + 1 + end];
                        let cleaned = path.replace("\\\\", "/").replace('\\', "/");
                        libs.push(PathBuf::from(cleaned));
                    }
                }
            }
        }
    }
    libs
}

#[command]
pub async fn detect_install() -> Result<Option<GameInstall>, String> {
    for steam in steam_candidates() {
        let mut libraries = vec![steam.clone()];
        libraries.extend(parse_libraryfolders(&steam.join("steamapps/libraryfolders.vdf")));
        for lib in libraries {
            let root = lib.join("steamapps/common/Propnight");
            if is_game_root(&root) {
                return Ok(Some(GameInstall {
                    exe: root.join(EXE_RELATIVE),
                    root,
                    valid: true,
                }));
            }
        }
    }
    Ok(None)
}

pub fn enable_mods(root: &Path, mods: &[String]) -> Result<(), String> {
    let win64 = root.join("Propnight/Binaries/Win64");
    let mods_dir = win64.join("Mods");
    std::fs::create_dir_all(&mods_dir).map_err(|e| e.to_string())?;
    let path = mods_dir.join("mods.txt");
    let mut lines: Vec<String> = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };
    for m in mods {
        let found = lines.iter_mut().find(|l| {
            l.split(':').next().unwrap_or("").trim().eq_ignore_ascii_case(m)
        });
        if let Some(line) = found {
            *line = format!("{m} : 1");
        } else {
            lines.push(format!("{m} : 1"));
        }
    }
    std::fs::write(&path, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
pub async fn set_identity(root: String, name: String, steam_id: String) -> Result<(), String> {
    let root = PathBuf::from(root);
    let dir = root.join("Engine/Binaries/ThirdParty/Steamworks/Steamv147/Win64/steam_settings");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("force_account_name.txt"), &name).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("force_steamid.txt"), &steam_id).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("steam_appid.txt"), "1549180").map_err(|e| e.to_string())?;
    let dll_dir = root.join("Engine/Binaries/ThirdParty/Steamworks/Steamv147/Win64");
    let _ = std::fs::write(dll_dir.join("steam_appid.txt"), "1549180");
    Ok(())
}


/// Launch via Proton and wait for the process to exit.

fn force_load_goldberg(compat: &Path, dll: &Path) {
    let pfx = compat.join("pfx");
    let sys32 = pfx.join("drive_c/windows/system32");
    let _ = std::fs::create_dir_all(&sys32);
    if dll.exists() {
        let _ = std::fs::copy(dll, sys32.join("steam_api64.dll"));
    }
    let reg = pfx.join("user.reg");
    let block = "\n[Software\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Windows] 1234567890\n#time=1d8c0c0c0c0c0c0\n\"AppInit_DLLs\"=\"C:\\\\windows\\\\system32\\\\steam_api64.dll\"\n\"LoadAppInit_DLLs\"=dword:00000001\n\"RequireSignedAppInit_DLLs\"=dword:00000000\n";
    let existing = std::fs::read_to_string(&reg).unwrap_or_else(|_| "WINE REGISTRY Version 2\n".into());
    let cleaned: String = existing.lines().filter(|l| !l.contains("AppInit_DLLs") && !l.contains("LoadAppInit_DLLs") && !l.contains("RequireSignedAppInit")).collect::<Vec<_>>().join("\n");
    let _ = std::fs::write(&reg, cleaned + block);
}

fn find_runtime() -> Option<PathBuf> {
    let mut bases = Vec::new();
    if let Some(home) = home_dir() {
        for p in [
            home.join(".local/share/Steam/steamapps/common"),
            home.join(".steam/steam/steamapps/common"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common"),
        ] {
            bases.push(p);
        }
    }
    for steam in steam_candidates() {
        bases.push(steam.join("steamapps/common"));
    }
    for base in bases {
        for name in ["SteamLinuxRuntime_sniper", "SteamLinuxRuntime_soldier", "SteamLinuxRuntime"] {
            let entry = base.join(name).join("_v2-entry-point");
            if entry.exists() { return Some(entry); }
        }
    }
    None
}


fn ge_proton_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.data_dir().join("PropnightRevival").join("GE-Proton"))
        .unwrap_or_else(|| PathBuf::from("GE-Proton"))
}

/// Download and extract the latest GE-Proton into the XDG data dir.
/// Returns the directory that contains the `proton` script.
fn find_existing_ge() -> Option<PathBuf> {
    let dest = ge_proton_dir();
    if dest.join("proton").exists() {
        return Some(dest);
    }
    // Also accept a versioned folder left by a previous extract
    if let Some(parent) = dest.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir()
                    && p.file_name().and_then(|n| n.to_str()).unwrap_or("").starts_with("GE-Proton")
                    && p.join("proton").exists()
                {
                    return Some(p);
                }
            }
        }
    }
    None
}

async fn ensure_ge_proton(app: Option<&tauri::AppHandle>) -> Result<PathBuf, String> {
    if let Some(existing) = find_existing_ge() {
        return Ok(existing);
    }
    let dest = ge_proton_dir();
    if let Some(app) = app {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.emit("state", &serde_json::json!({
                "play": { "title": "DOWNLOADING", "sub": "Fetching GE-Proton…", "busy": true, "disabled": true, "progress": -1 }
            }));
        }
    }
    let client = reqwest::Client::builder()
        .user_agent("PropnightRevivalLauncher/linux")
        .build()
        .map_err(|e| e.to_string())?;
    let rel: serde_json::Value = client
        .get("https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases/latest")
        .send().await.map_err(|e| e.to_string())?
        .json().await.map_err(|e| e.to_string())?;
    let asset = rel["assets"].as_array().and_then(|a| {
        a.iter().find(|x| x["name"].as_str().unwrap_or("").contains("x86_64.tar.gz")
            && !x["name"].as_str().unwrap_or("").contains("sha"))
    }).ok_or("no GE-Proton x86_64 asset")?;
    let url = asset["browser_download_url"].as_str().ok_or("no download url")?;
    let name = asset["name"].as_str().unwrap_or("GE-Proton.tar.gz");
    if let Some(app) = app {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.emit("state", &serde_json::json!({
                "play": { "title": "DOWNLOADING", "sub": format!("Downloading {name}"), "busy": true, "disabled": true, "progress": -1 }
            }));
        }
    }
    let bytes = client.get(url).send().await.map_err(|e| e.to_string())?
        .bytes().await.map_err(|e| e.to_string())?;
    let tmp = dest.with_extension("tar.gz");
    if let Some(parent) = tmp.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    // Extract. GE-Proton archives contain a top-level folder.
    let status = std::process::Command::new("tar")
        .args(["-xzf", tmp.to_str().unwrap_or(""), "-C"])
        .arg(tmp.parent().unwrap_or(Path::new(".")))
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() { return Err("failed to extract GE-Proton".into()); }
    let _ = std::fs::remove_file(&tmp);
    // Find the extracted folder and move/rename to dest
    if let Some(parent) = tmp.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() && p.file_name().and_then(|n| n.to_str()).unwrap_or("").starts_with("GE-Proton") && p.join("proton").exists() {
                    if p != dest {
                        let _ = std::fs::rename(&p, &dest);
                    }
                    break;
                }
            }
        }
    }
    if !dest.join("proton").exists() {
        return Err("GE-Proton extracted but proton script not found".into());
    }
    Ok(dest)
}

fn find_umu() -> Option<PathBuf> {
    // which first
    if let Ok(out) = Command::new("which").arg("umu-run").output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() && PathBuf::from(&p).exists() { return Some(PathBuf::from(p)); }
        }
    }
    // Common install locations (AppImage PATH is often minimal)
    if let Some(home) = home_dir() {
        for p in [
            home.join(".local/bin/umu-run"),
            PathBuf::from("/usr/bin/umu-run"),
            PathBuf::from("/usr/local/bin/umu-run"),
        ] {
            if p.exists() { return Some(p); }
        }
    }
    None
}

pub async fn launch_and_wait(root: &str) -> Result<(), String> {
    let root = PathBuf::from(root);
    let exe = root.join(EXE_RELATIVE);
    if !exe.exists() {
        return Err("Propnight.exe not found".into());
    }
    let ge = ensure_ge_proton(None).await?;
    let proton = ge.join("proton");
    let compat = find_or_create_compat(&root)?;
    // Proton user_settings.py forces the Goldberg DLL even if the env is dropped.
    let user_settings = r#"# Force Goldberg steam_api64 (native only) and disable real steamclient
import os
os.environ["WINEDLLOVERRIDES"] = "steam_api64=n,b;dwmapi=n,b"
"#;
    let _ = std::fs::write(ge.join("user_settings.py"), user_settings);
    let _ = std::fs::create_dir_all(&compat);
    let _ = std::fs::write(compat.join("user_settings.py"), user_settings);
    let steam = steam_candidates().into_iter().next().unwrap_or_else(|| PathBuf::from("."));
    let log_path = directories::BaseDirs::new()
        .map(|b| b.config_dir().join("PropnightRevival").join("proton.log"))
        .unwrap_or_else(|| PathBuf::from("proton.log"));
    if let Some(parent) = log_path.parent() { let _ = std::fs::create_dir_all(parent); }

    // Prefer umu-run (designed to run Proton outside Steam with the full runtime).
    // Fall back to SteamLinuxRuntime wrapper, then raw proton.
    let proton_dir = ge.clone();
    let mut cmd = if let Some(umu) = find_umu() {
        let mut c = Command::new(umu);
        // umu wants the directory that contains the proton script.
        // Also set STORE=steam so it treats this as the Steam app.
        c.env("PROTONPATH", &proton_dir);
        c.env("GAMEID", "umu-1549180");
        // Do not set STORE=steam — that makes umu talk to the Steam client
        // and the game sticks on Initializing Matchmaking.
        c.env("WINEPREFIX", compat.join("pfx"));
        c.arg(&exe);
        c
    } else if let Some(rt) = find_runtime() {
        let mut c = Command::new(rt);
        c.arg("--verb=run").arg("--").arg(&proton).arg("run").arg(&exe);
        c
    } else {
        let mut c = Command::new("python3");
        c.arg(&proton).arg("run").arg(&exe);
        c
    };
    // Unreal loads steam_api64.dll from the Steamworks path, but Wine often
    // fails to find it unless it is also next to the exe. Copy Goldberg there
    // and force Wine to use the native (game) DLL.
    let steamworks = root.join("Engine/Binaries/ThirdParty/Steamworks/Steamv147/Win64");
    let src_dll = steamworks.join("steam_api64.dll");
    let win64 = exe.parent().unwrap_or(&root).to_path_buf();
    let dst_dll = win64.join("steam_api64.dll");
    // Same as the original: enable revival mods in Mods/mods.txt before launch.
    let _ = enable_mods(&root, &["PropnightLoginBypasser".into(), "PropnightLocalMap".into()]);
    let ue4ss = win64.join("UE4SS.dll");
    let dwmapi = win64.join("dwmapi.dll");
    if !ue4ss.exists() || !dwmapi.exists() {
        return Err("UE4SS.dll or dwmapi.dll missing next to Propnight.exe. Run PLAY so the updater installs the revival mods.".into());
    }
    if !src_dll.exists() {
        return Err("Goldberg steam_api64.dll missing in Steamworks folder. Run PLAY once so the updater installs it.".into());
    }
    std::fs::copy(&src_dll, &dst_dll).map_err(|e| format!("failed to copy Goldberg DLL: {e}"))?;
    let src_settings = steamworks.join("steam_settings");
    let dst_settings = win64.join("steam_settings");
    if src_settings.exists() {
        let _ = std::fs::create_dir_all(&dst_settings);
        if let Ok(entries) = std::fs::read_dir(&src_settings) {
            for e in entries.flatten() {
                let _ = std::fs::copy(e.path(), dst_settings.join(e.file_name()));
            }
        }
    }
    // Always refresh Goldberg config on this launch (do not depend on settings).
    let settings = crate::settings::load_settings().await.unwrap_or_default();
    let name = settings.username.clone().unwrap_or_else(|| "Player".into());
    let sid = settings.steam_id.clone().unwrap_or_else(|| "76561197960265728".into());
    for dir in [&steamworks.join("steam_settings"), &win64.join("steam_settings")] {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("force_account_name.txt"), &name);
        let _ = std::fs::write(dir.join("force_steamid.txt"), &sid);
        let _ = std::fs::write(dir.join("steam_appid.txt"), "1549180");
        let _ = std::fs::write(dir.join("disable_overlay.txt"), "");
    }
    let _ = std::fs::write(win64.join("steam_appid.txt"), "1549180");
    let _ = std::fs::write(steamworks.join("steam_appid.txt"), "1549180");
    // Game crashes at +0x181A576 without ever calling LoadLibrary("steam_api64").
    // Force-load Goldberg via AppInit_DLLs so it is in the process before that call.
    force_load_goldberg(&compat, &dst_dll);
    // Game may live on a separate mount (e.g. /home/user/storage1).
    // Pressure-vessel refuses "parent for game drive" unless that path is shared.
    let share = root.parent().unwrap_or(&root).to_string_lossy().to_string();
    cmd.env("STEAM_COMPAT_DATA_PATH", &compat)
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam)
        .env("STEAM_COMPAT_INSTALL_PATH", &root)
        .env("STEAM_COMPAT_LIBRARY_PATHS", &share)
        .env("STEAM_COMPAT_APP_ID", "1549180")
        .env("SteamAppId", "1549180")
        .env("SteamGameId", "1549180")
        .env("PRESSURE_VESSEL_FILESYSTEMS_RW", &share)
        .env("WINEDLLOVERRIDES", "steam_api64=n,b;dwmapi=n,b")
        .env("PROTON_LOG", "1")
        .env("WINEDEBUG", "+loaddll,+module,+seh")
        .env("UMU_LOG", "1")
        .env("DXVK_LOG_LEVEL", "info")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONSAFEPATH")
        .env_remove("PYTHONSTARTUP")
        .current_dir(exe.parent().unwrap_or(&root))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd.spawn().map_err(|e| format!("failed to spawn Proton: {e}"))?;
    let output = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await.map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let dll_info = {
        let p = win64.join("steam_api64.dll");
        if p.exists() {
            let hash = crate::crypto::sha256_file(&p).unwrap_or_else(|_| "?".into());
            format!("steam_api64={} sha256={} (expect 0cfe547e…)", p.display(), hash)
        } else {
            "steam_api64=MISSING next to exe".into()
        }
    };
    let combined = format!("PROTONPATH={}\n{}\n{}\n{}", proton_dir.display(), dll_info, String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let _ = std::fs::write(&log_path, &combined);

    if !output.status.success() {
        let tail: String = combined.trim().chars().rev().take(600).collect::<String>().chars().rev().collect();
        return Err(format!("Proton exited {}. Log: {}. …{}", output.status, log_path.display(), tail));
    }
    Ok(())
}

/// Launch Propnight.exe via Proton (or Steam if preferred).
/// Prefers an existing Proton prefix under steamapps/compatdata if present.
#[command]
pub async fn launch_game(root: String) -> Result<String, String> {
    let root = PathBuf::from(&root);
    let exe = root.join(EXE_RELATIVE);
    if !exe.exists() {
        return Err("Propnight.exe not found".into());
    }

    // Try to locate a Proton binary (common locations)
    let proton = find_proton_public().ok_or("Could not find a Proton installation. Install Proton-GE or use Steam's Proton.")?;

    // Prefer the game's own compatdata if it exists, otherwise create a dedicated one
    let compat = find_or_create_compat(&root)?;

    let status = Command::new(&proton)
        .env("STEAM_COMPAT_DATA_PATH", &compat)
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_candidates().first().unwrap_or(&PathBuf::from(".")))
        .arg("run")
        .arg(&exe)
        .current_dir(exe.parent().unwrap_or(&root))
        .spawn()
        .map_err(|e| format!("failed to spawn Proton: {e}"))?;

    Ok(format!("Launched via Proton (pid {})", status.id()))
}

pub fn find_proton_public() -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut bases = Vec::new();
    if let Some(home) = home_dir() {
        // Explicit Proton Experimental locations first
        for p in [
            home.join(".local/share/Steam/steamapps/common/Proton - Experimental/proton"),
            home.join(".steam/steam/steamapps/common/Proton - Experimental/proton"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/Proton - Experimental/proton"),
        ] {
            if p.exists() { found.push(p); }
        }
        // Extra Steam libraries (e.g. /home/xan/storage1/SteamLibrary)
        for steam in steam_candidates() {
            if let Ok(libs) = std::fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")) {
                for lib in parse_libraryfolders(&steam.join("steamapps/libraryfolders.vdf")) {
                    let p = lib.join("steamapps/common/Proton - Experimental/proton");
                    if p.exists() { found.push(p); }
                    bases.push(lib.join("steamapps/common"));
                    bases.push(lib.join("compatibilitytools.d"));
                }
            }
        }
        bases.push(home.join(".steam/steam/compatibilitytools.d"));
        bases.push(home.join(".local/share/Steam/compatibilitytools.d"));
        bases.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d"));
        bases.push(home.join(".steam/steam/steamapps/common"));
        bases.push(home.join(".local/share/Steam/steamapps/common"));
        bases.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common"));
    }
    for steam in steam_candidates() {
        bases.push(steam.join("compatibilitytools.d"));
        bases.push(steam.join("steamapps/common"));
    }
    for base in bases {
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let proton = entry.path().join("proton");
                if proton.exists() { found.push(proton); }
                if let Ok(sub) = std::fs::read_dir(entry.path()) {
                    for s in sub.flatten() {
                        let p = s.path().join("proton");
                        if p.exists() { found.push(p); }
                    }
                }
            }
        }
    }
    if let Ok(out) = Command::new("which").arg("proton").output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() { found.push(PathBuf::from(p)); }
        }
    }
    // Prefer Proton Experimental by directory name, then Proton-GE, then others.
    // Never prefer umu unless nothing else exists.
    fn rank(p: &PathBuf) -> u8 {
        let s = p.to_string_lossy().to_lowercase();
        if s.contains("proton - experimental") || s.contains("proton-experimental") || s.contains("/experimental/") {
            0
        } else if s.contains("umu") {
            9  // last resort
        } else if s.contains("ge-") || s.contains("proton-ge") || s.contains("proton ge") {
            1
        } else if s.contains("proton") {
            2
        } else {
            3
        }
    }
    found.sort_by_key(rank);
    found.into_iter().next()
}

fn find_or_create_compat(game_root: &Path) -> Result<PathBuf, String> {
    // Prefer the prefix Steam already uses for 1549180 (works from the Steam UI).
    let mut candidates = Vec::new();
    for steam in steam_candidates() {
        candidates.push(steam.join("steamapps/compatdata/1549180"));
        if let Ok(libs) = std::fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")) {
            for lib in parse_libraryfolders(&steam.join("steamapps/libraryfolders.vdf")) {
                candidates.push(lib.join("steamapps/compatdata/1549180"));
            }
        }
    }
    // Also next to the game library itself
    if let Some(lib) = game_root.parent().and_then(|p| p.parent()) {
        candidates.push(lib.join("compatdata/1549180"));
    }
    for c in candidates {
        if c.join("pfx").exists() || c.exists() {
            return Ok(c);
        }
    }
    let fallback = game_root.join(".proton-prefix");
    std::fs::create_dir_all(&fallback).map_err(|e| e.to_string())?;
    Ok(fallback)
}

// Helper for home / data dirs
fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}
fn data_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.data_dir().to_path_buf())
}
