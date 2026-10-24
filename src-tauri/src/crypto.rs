use base64::{engine::general_purpose::STANDARD, Engine};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use p256::EncodedPoint;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const API_BASE: &str = "https://api.propnightrevival.online";
pub const MANIFEST_URL: &str = "https://api.propnightrevival.online/updates/manifest.json";
pub const SIGNATURE_URL: &str = "https://api.propnightrevival.online/updates/manifest.sig";
/// ECDSA P-256 public key (uncompressed X||Y, base64) from the original launcher.
pub const UPDATE_PUBLIC_KEY: &str = "DAShBnxG+tOeg2yvXcmhm85O8Myyv37T7HVVd7ld/YEViijaCNPNj8CFT+zovWcvrj9JdV8RKJga8HNS3o+zkg==";

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(hex::encode(hasher.finalize()))
}

pub fn sha256_bytes(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Verify ECDSA-P256 signature (raw 64-byte r||s) over the manifest bytes.
pub fn verify_manifest(data: &[u8], signature: &[u8]) -> Result<(), String> {
    if signature.len() != 64 {
        return Err("signature must be 64 bytes (r||s)".into());
    }
    let xy = STANDARD
        .decode(UPDATE_PUBLIC_KEY)
        .map_err(|e| format!("public key decode: {e}"))?;
    if xy.len() != 64 {
        return Err("public key must be 64 bytes".into());
    }
    // SEC1 uncompressed point: 0x04 || X || Y
    let mut point = vec![0x04];
    point.extend_from_slice(&xy);
    let encoded = EncodedPoint::from_bytes(&point).map_err(|e| format!("point: {e}"))?;
    let vk = VerifyingKey::from_encoded_point(&encoded).map_err(|e| format!("verifying key: {e}"))?;
    let sig = Signature::from_bytes(signature.into()).map_err(|e| format!("signature parse: {e}"))?;
    vk.verify(data, &sig)
        .map_err(|_| "manifest signature verification failed".to_string())
}
