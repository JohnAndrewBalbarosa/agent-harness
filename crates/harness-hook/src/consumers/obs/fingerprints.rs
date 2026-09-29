//! obs.py `digest` / `secret_key` and hook.py's `os.urandom(16).hex()` trace ids.
//!
//! The HMAC key is the SAME `var/hmac.key` file obs.py uses next to the spool, so keyed
//! fingerprints (session, prompt) stay identical whichever implementation wrote them.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const KEY_BYTES: usize = 32;

/// `hashlib.sha256(value.encode()).hexdigest()` (obs.py `digest(value)`).
#[inline]
pub fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// `hmac.new(key, value.encode(), sha256).hexdigest()` (obs.py `digest(value, keyed=True)`).
pub fn hmac_sha256_hex(key: &[u8], value: &str) -> anyhow::Result<String> {
    // HMAC accepts keys of any length; the error branch exists only in the generic API.
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("invalid HMAC key length"))?;
    mac.update(value.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// obs.py `secret_key()`: read `hmac.key`, creating 32 random bytes (mode 0600) when missing.
///
/// Deviation: creation uses `create_new`, so two first-time writers never overwrite each other's key.
pub fn load_or_create_key(key_path: &Path) -> io::Result<Vec<u8>> {
    if let Some(directory) = key_path.parent() {
        fs::create_dir_all(directory)?;
    }
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(key_path)
    {
        Ok(mut file) => {
            file.write_all(&random_bytes(KEY_BYTES))?;
            restrict_to_owner(key_path);
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    fs::read(key_path)
}

/// `os.urandom(16).hex()`.
#[inline]
pub fn random_hex(byte_count: usize) -> String {
    hex::encode(random_bytes(byte_count))
}

/// OS-random bytes via v4 UUIDs (getrandom) condensed through SHA-256 blocks: each 32-byte block
/// hashes three v4 UUIDs (366 random bits), so every returned byte is fully keyed by OS entropy.
fn random_bytes(byte_count: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(byte_count);
    while bytes.len() < byte_count {
        let mut hasher = Sha256::new();
        for _ in 0..3 {
            hasher.update(Uuid::new_v4().as_bytes());
        }
        bytes.extend_from_slice(&hasher.finalize());
    }
    bytes.truncate(byte_count);
    bytes
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_match_python_hashlib_and_hmac() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // hmac.new(b"key", b"The quick brown fox jumps over the lazy dog", hashlib.sha256).hexdigest()
        assert_eq!(
            hmac_sha256_hex(b"key", "The quick brown fox jumps over the lazy dog").unwrap(),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn key_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("var").join("hmac.key");
        let first = load_or_create_key(&path).unwrap();
        let second = load_or_create_key(&path).unwrap();
        assert_eq!(first.len(), KEY_BYTES);
        assert_eq!(first, second);
    }

    #[test]
    fn random_hex_has_requested_width() {
        let trace = random_hex(16);
        assert_eq!(trace.len(), 32);
        assert_ne!(trace, random_hex(16));
    }
}
