use crate::crypto::{sha256_bytes, sha256_file, verify_manifest, MANIFEST_URL, SIGNATURE_URL};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::command;

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
    download_file_progress(client, url, target, expected_sha, &mut |_| {}).await
}

async fn download_file_progress(
    client: &Client,
    url: &str,
    target: &Path,
    expected_sha: &str,
    progress: &mut dyn FnMut(f64),
) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);
    let mut stream = resp.bytes_stream();
    let mut got = Vec::new();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        got.extend_from_slice(&chunk);
        if total > 0 { progress(got.len() as f64 / total as f64); }
    }
    std::fs::write(target, &got).map_err(|e| e.to_string())?;
    let sha = sha256_file(target)?;
    if sha != expected_sha {
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
pub async fn plan_updates(game_root: &str) -> Result<(String, u32), String> {
    let client = Client::builder()
        .user_agent("PropnightRevivalLauncher/linux")
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
        .user_agent("PropnightRevivalLauncher/linux")
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

    if todo.is_empty() && manifest.exe_patch.is_none() {
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

    if let Some(patch) = &manifest.exe_patch {
        apply_exe_patch(&root, patch)?;
    }

    crate::game::enable_mods(&root, &manifest.mods_enabled)?;

    Ok(UpdateResult {
        up_to_date: todo.is_empty(),
        files_changed: todo.len() as u32,
        message: format!("Updated {} file(s) to manifest {}", todo.len(), manifest.version),
        version: Some(manifest.version),
    })
}

fn apply_exe_patch(root: &Path, patch: &ExePatch) -> Result<(), String> {
    let exe = local_path(root, &patch.path)?;
    if !exe.exists() {
        return Err("Propnight.exe not found".into());
    }
    let current_sha = sha256_file(&exe)?;
    if current_sha == patch.patched_sha256 {
        return Ok(());
    }
    if !patch.accepted_sha256.iter().any(|s| s == &current_sha) {
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
    Ok(())
}
