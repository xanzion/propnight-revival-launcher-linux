use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::command;

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
    Ok(())
}


/// Launch via Proton and wait for the process to exit.
pub async fn launch_and_wait(root: &str) -> Result<(), String> {
    let root = PathBuf::from(root);
    let exe = root.join(EXE_RELATIVE);
    if !exe.exists() {
        return Err("Propnight.exe not found".into());
    }
    let proton = find_proton_public().ok_or("Could not find a Proton installation. Install Proton Experimental or Proton-GE through Steam.")?;
    let compat = find_or_create_compat(&root)?;
    let steam = steam_candidates().into_iter().next().unwrap_or_else(|| PathBuf::from("."));

    // Proton Experimental's "proton" is a Python script (shebang python3).
    // Running it with bash makes bash interpret "import" as ImageMagick.
    // Prefer python3 explicitly; fall back to executing the script directly.
    let mut cmd = Command::new("python3");
    cmd.arg(&proton)
        .arg("run")
        .arg(&exe)
        .env("STEAM_COMPAT_DATA_PATH", &compat)
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam)
        .env("STEAM_COMPAT_APP_ID", "1549180")
        .env("SteamAppId", "1549180")
        .env("SteamGameId", "1549180")
        .current_dir(exe.parent().unwrap_or(&root))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn Proton ({}): {e}", proton.display()))?;
    let output = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        let msg = if !err.trim().is_empty() { err } else { out };
        return Err(format!("Proton exited {}: {}", output.status, msg.trim().chars().take(300).collect::<String>()));
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
    // Look for existing Propnight compatdata (AppID 1549180)
    for steam in steam_candidates() {
        let compat = steam.join("steamapps/compatdata/1549180");
        if compat.exists() {
            return Ok(compat);
        }
    }
    // Fallback: dedicated prefix next to the game
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
