use crate::crypto::{sha256_bytes, sha256_file, verify_manifest, MANIFEST_URL, SIGNATURE_URL};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{command, Emitter, Manager};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub dlc: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExePatch {
    pub path: String,
    #[serde(default, rename = "originalSha256")]
    pub original_sha256: Option<String>,
    #[serde(rename = "patchedSha256")]
    pub patched_sha256: String,
    #[serde(rename = "acceptedSha256")]
    pub accepted_sha256: Vec<String>,
    pub patches: Vec<BytePatch>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BytePatch {
    pub offset: u64,
    pub original: String,
    pub replacement: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default, rename = "gameDlls")]
    pub game_dlls: serde_json::Value,
    #[serde(default, rename = "paksAllowed")]
    pub paks_allowed: Vec<String>,
    #[serde(default, rename = "modsAllowed")]
    pub mods_allowed: Vec<String>,

    pub version: String,
    #[serde(rename = "filesUrl")]
    pub files_url: String,
    pub files: Vec<ManifestFile>,
    #[serde(default, rename = "exePatch")]
    pub exe_patch: Option<ExePatch>,
    #[serde(default, rename = "modsEnabled")]
    pub mods_enabled: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateResult {
    pub up_to_date: bool,
    pub files_changed: u32,
    pub message: String,
    pub version: Option<String>,
}

fn local_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let cleaned = relative.replace('\\', "/");
    let full = root.join(&cleaned);
    let canon_root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    let full_canon = full.canonicalize().unwrap_or(full.clone());
    if !full_canon.starts_with(&canon_root) && !full.starts_with(root) {
        return Err(format!("path escapes game root: {relative}"));
    }
    Ok(full)
}

fn backup(root: &Path, relative: &str) -> Result<(), String> {
    let src = local_path(root, relative)?;
    if !src.exists() {
        return Ok(());
    }
    let backup = root.join("revival-backups").join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    if backup.exists() {
        return Ok(());
    }
    if let Some(parent) = backup.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(&src, &backup).map_err(|e| e.to_string())?;
    Ok(())
}

async fn download_file(client: &Client, url: &str, target: &Path, expected_sha: &str) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?;
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    std::fs::write(target, &bytes).map_err(|e| e.to_string())?;
    let got = sha256_file(target)?;
    if got != expected_sha {
        let _ = std::fs::remove_file(target);
        return Err(format!("checksum mismatch for {target:?}"));
    }
    Ok(())
}

pub async fn load_manifest(client: &Client) -> Result<Manifest, String> {
    let data = client
        .get(format!("{MANIFEST_URL}?t={}", chrono_like_ticks()))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let sig_b64 = client
        .get(format!("{SIGNATURE_URL}?t={}", chrono_like_ticks()))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let sig = STANDARD
        .decode(sig_b64.trim())
        .map_err(|e| format!("sig decode: {e}"))?;
    verify_manifest(&data, &sig)?;
    let manifest: Manifest = serde_json::from_slice(&data).map_err(|e| format!("manifest parse: {e}"))?;
    Ok(manifest)
}

fn chrono_like_ticks() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}


/// Plan only: count files that differ from the signed manifest. Does not download or patch.
/// Local config we own. Do not let the manifest overwrite or flag it.
fn is_local_override(path: &str) -> bool {
    path.replace('\\', "/").ends_with("UE4SS-settings.ini")
}

pub async fn plan_updates(game_root: &str) -> Result<(String, u32), String> {
    let client = Client::builder()
        .user_agent("PropnightRevivalLauncher/1.46.8")
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let manifest = load_manifest(&client).await?;
    let root = PathBuf::from(game_root);
    if !root.exists() {
        return Ok((manifest.version, 0));
    }
    let mut count = 0u32;
    for f in &manifest.files {
        if is_local_override(&f.path) { continue; }
        let path = local_path(&root, &f.path)?;
        let needs = if path.exists() {
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == f.size => {
                    let sha = sha256_file(&path)?;
                    sha != f.sha256
                }
                _ => true,
            }
        } else {
            true
        };
        if needs {
            count += 1;
        }
    }
    Ok((manifest.version, count))
}

#[command]
pub async fn check_updates(game_root: String) -> Result<UpdateResult, String> {
    let client = Client::builder()
        .user_agent("PropnightRevivalLauncher/1.46.8")
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let manifest = load_manifest(&client).await?;
    let root = PathBuf::from(&game_root);
    if !root.exists() {
        return Err("game root does not exist".into());
    }

    let mut todo = Vec::new();
    for f in &manifest.files {
        if f.dlc.is_some() { continue; }  // DLC files are installed only via install_dlc
        if is_local_override(&f.path) { continue; }
        let path = local_path(&root, &f.path)?;
        let needs = if path.exists() {
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == f.size => {
                    let sha = sha256_file(&path)?;
                    sha != f.sha256
                }
                _ => true,
            }
        } else {
            true
        };
        if needs {
            todo.push(f);
        }
    }

    let mut patched = false;
    if let Some(patch) = &manifest.exe_patch {
        patched = apply_exe_patch(&root, patch)?;
    }
    ensure_manifest_file(&client, &root, &manifest, "Propnight/Content/Paks/Propnight-WindowsNoEditor_RevivalFixes_P.pak").await?;

    if todo.is_empty() && !patched {
        return Ok(UpdateResult {
            up_to_date: true,
            files_changed: 0,
            message: format!("Up to date (manifest {})", manifest.version),
            version: Some(manifest.version),
        });
    }

    for f in &todo {
        let path = local_path(&root, &f.path)?;
        let tmp = path.with_extension("download");
        let url = format!("{}{}", manifest.files_url, f.sha256);
        download_file(&client, &url, &tmp, &f.sha256).await?;
        backup(&root, &f.path)?;
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }

    // Explicitly ensure Goldberg steam_api64.dll is present and correct.
    // The game crashes on matchmaking if this is missing or is the original Steam DLL.
    let steam_api = "Engine/Binaries/ThirdParty/Steamworks/Steamv147/Win64/steam_api64.dll";
    if let Some(f) = manifest.files.iter().find(|f| f.path.replace('\\', "/") == steam_api) {
        let path = local_path(&root, steam_api)?;
        let ok = path.exists()
            && std::fs::metadata(&path).map(|m| m.len() == f.size).unwrap_or(false)
            && sha256_file(&path).map(|h| h == f.sha256).unwrap_or(false);
        if !ok {
            let tmp = path.with_extension("download");
            let url = format!("{}{}", manifest.files_url, f.sha256);
            download_file(&client, &url, &tmp, &f.sha256).await?;
            backup(&root, steam_api)?;
            if path.exists() { std::fs::remove_file(&path).ok(); }
            std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
            tracing::info!("reinstalled steam_api64.dll (Goldberg)");
        }
    }

    crate::game::enable_mods(&root, &manifest.mods_enabled)?;
    if !manifest.mods_allowed.is_empty() {
        let mods_dir = root.join("Propnight/Binaries/Win64/Mods");
        if let Ok(rd) = std::fs::read_dir(&mods_dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if ent.path().is_dir() && !manifest.mods_allowed.iter().any(|m| m == &name) && name != "mods.txt" {
                    tracing::warn!("mod not in modsAllowed: {name}");
                }
            }
        }
    }

    Ok(UpdateResult {
        up_to_date: todo.is_empty(),
        files_changed: todo.len() as u32 + if patched { 1 } else { 0 },
        message: format!("Updated {} file(s){} to manifest {}", todo.len(), if patched { " and Propnight.exe" } else { "" }, manifest.version),
        version: Some(manifest.version),
    })
}


/// Download only the files belonging to a DLC pack.
pub async fn install_dlc(game_root: &str, dlc_id: &str, app: &tauri::AppHandle) -> Result<u32, String> {
    let client = Client::builder()
        .user_agent("PropnightRevivalLauncher/1.46.8")
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(|e| e.to_string())?;
    let manifest = load_manifest(&client).await?;
    let root = PathBuf::from(game_root);
    if !root.exists() {
        return Err("game root does not exist".into());
    }
    let files: Vec<_> = manifest.files.iter().filter(|f| f.dlc.as_deref() == Some(dlc_id)).collect();
    if files.is_empty() {
        return Err(format!("no files for DLC {dlc_id}"));
    }
    let total = files.len();
    let mut done = 0u32;
    for f in files {
        done += 1;
        let path = local_path(&root, &f.path)?;
        // skip if already correct
        let needs = if path.exists() {
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == f.size => {
                    let sha = sha256_file(&path)?;
                    sha != f.sha256
                }
                _ => true,
            }
        } else {
            true
        };
        if !needs { continue; }
        let tmp = path.with_extension("download");
        let url = format!("{}{}", manifest.files_url, f.sha256);
        // progress
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.emit("state", &serde_json::json!({
                "play": { "title": "DOWNLOADING", "sub": format!("{} ({}/{})", std::path::Path::new(&f.path).file_name().and_then(|n| n.to_str()).unwrap_or(&f.path), done, total), "busy": true, "disabled": true, "progress": done as f64 / total as f64 }
            }));
        }
        download_file(&client, &url, &tmp, &f.sha256).await?;
        backup(&root, &f.path)?;
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(done)
}

/// Download a manifest file if it is missing or the hash differs. No-op if absent from the manifest.
async fn ensure_manifest_file(client: &Client, root: &Path, manifest: &Manifest, relative: &str) -> Result<(), String> {
    let Some(f) = manifest.files.iter().find(|f| f.path.replace('\\', "/") == relative) else { return Ok(()) };
    let path = local_path(root, relative)?;
    let ok = path.exists()
        && std::fs::metadata(&path).map(|m| m.len() == f.size).unwrap_or(false)
        && sha256_file(&path).map(|h| h == f.sha256).unwrap_or(false);
    if ok { return Ok(()) }
    let tmp = path.with_extension("download");
    let url = format!("{}{}", manifest.files_url, f.sha256);
    download_file(client, &url, &tmp, &f.sha256).await?;
    backup(root, relative)?;
    if path.exists() { std::fs::remove_file(&path).ok(); }
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    tracing::info!("installed {relative}");
    Ok(())
}

/// Apply the signed exe patch. Returns true if the file was rewritten.
/// A new patch set is applied even when the exe was patched before: restore the
/// backed-up original first so the new bytes land on a known base.
fn apply_exe_patch(root: &Path, patch: &ExePatch) -> Result<bool, String> {
    let exe = local_path(root, &patch.path)?;
    if !exe.exists() {
        return Err("Propnight.exe not found".into());
    }
    let current_sha = sha256_file(&exe)?;
    if current_sha == patch.patched_sha256 {
        return Ok(false);
    }
    let original_sha = patch.original_sha256.clone().unwrap_or_default();
    if current_sha != original_sha {
        let backup = root.join("revival-backups").join(patch.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if backup.exists() {
            std::fs::copy(&backup, &exe).map_err(|e| format!("restore original exe: {e}"))?;
        }
    }
    let current_sha = sha256_file(&exe)?;
    if current_sha == patch.patched_sha256 {
        return Ok(false);
    }
    if !patch.accepted_sha256.iter().any(|s| s == &current_sha) && current_sha != original_sha {
        return Err("Propnight.exe is not a known Steam build. Verify game files in Steam, then retry.".into());
    }
    let mut bytes = std::fs::read(&exe).map_err(|e| e.to_string())?;
    for p in &patch.patches {
        let original = hex::decode(&p.original).map_err(|e| e.to_string())?;
        let replacement = hex::decode(&p.replacement).map_err(|e| e.to_string())?;
        let offset = p.offset as usize;
        if offset + original.len() > bytes.len() {
            return Err(format!("patch offset out of range ({})", p.name.as_deref().unwrap_or("?")));
        }
        let current = &bytes[offset..offset + original.len()];
        if current != original.as_slice() && current != replacement.as_slice() {
            return Err(format!(
                "unexpected bytes at patch {} (offset {})",
                p.name.as_deref().unwrap_or("?"),
                offset
            ));
        }
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
    }
    let final_sha = sha256_bytes(&bytes);
    if final_sha != patch.patched_sha256 {
        return Err("post-patch SHA-256 mismatch; nothing written".into());
    }
    backup(root, &patch.path)?;
    let tmp = exe.with_extension("download");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::remove_file(&exe).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &exe).map_err(|e| e.to_string())?;
    tracing::info!("Propnight.exe patched to {}", patch.patched_sha256);
    Ok(true)
}
