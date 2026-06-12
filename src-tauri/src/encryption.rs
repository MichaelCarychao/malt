// Per-note password-based encryption.
//
// Design goals:
//   - File on disk is still a single text file (Dropbox / Syncthing / git
//     remain happy). No sidecar files, no .enc extension change. Encrypted
//     notes use a magic-line envelope and are otherwise just .md.
//   - Strong enough crypto that a casual attacker who steals the file
//     can't decrypt it: AES-256-GCM authenticated encryption with an
//     Argon2id password-derived key. Salt + nonce baked into the envelope
//     so we don't need a sidecar.
//   - Each save uses a fresh nonce. We re-run the (expensive) Argon2 KDF
//     only when the salt changes, so re-saves with the same password reuse
//     the cached key in memory (handled by the caller, not here).
//
// On-disk envelope (single line, plus a trailing newline):
//
//     MALT-ENC-v1:<base64-standard-no-pad of: salt(16) || nonce(12) || ciphertext || tag(16)>
//
// The version tag is part of the magic so we can change KDF or AEAD in
// the future without breaking detection on old files.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine as _;
use rand::RngCore;

const MAGIC: &str = "MALT-ENC-v1:";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;

/// Cheap detector: a file is encrypted iff its first non-BOM bytes are the
/// magic prefix FOLLOWED BY a plausible envelope — one unbroken base64 run
/// of at least salt+nonce+tag (44 bytes -> 59 unpadded chars). The prefix
/// alone isn't enough: a note that merely *mentions* the magic string
/// ("MALT-ENC-v1: looks like this") would otherwise be treated as
/// encrypted and become unopenable (the indexer skips it, the editor
/// demands a password, decrypt fails). Doesn't allocate.
pub fn is_encrypted(content: &str) -> bool {
    let Some(rest) = content.trim_start_matches('\u{feff}').strip_prefix(MAGIC) else {
        return false;
    };
    let line = rest.lines().next().unwrap_or("").trim_end();
    const MIN_B64: usize = (SALT_LEN + NONCE_LEN + TAG_LEN) * 4 / 3; // 58 -> need 59
    line.len() > MIN_B64
        && line
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// Process-wide cache of derived keys. Argon2id is deliberately slow
/// (tens-to-hundreds of ms); without this, every autosave of an
/// encrypted note re-runs the full KDF. The cache key is a fast hash
/// of (salt || password) so two notes sharing a password+salt reuse
/// the work, and a wrong password never collides onto the right key.
/// Keys live only in memory; cleared on process exit. (The frontend's
/// per-vault / on-blur password clearing governs how long a note stays
/// unlocked; this cache is purely a CPU optimization underneath that.)
fn key_cache() -> &'static std::sync::Mutex<std::collections::HashMap<u64, [u8; KEY_LEN]>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<u64, [u8; KEY_LEN]>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn cache_key(password: &str, salt: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    salt.hash(&mut h);
    password.hash(&mut h);
    h.finish()
}

/// Derive a 32-byte AES key from `password` + `salt` using Argon2id with
/// library default parameters, memoized. Argon2id is the OWASP-
/// recommended password-hash function as of 2024.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], String> {
    let ck = cache_key(password, salt);
    if let Ok(cache) = key_cache().lock() {
        if let Some(k) = cache.get(&ck) {
            return Ok(*k);
        }
    }
    let argon2 = Argon2::default();
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| format!("argon2: {e}"))?;
    if let Ok(mut cache) = key_cache().lock() {
        cache.insert(ck, key);
    }
    Ok(key)
}

/// Pull the salt out of an existing envelope, so a re-save can reuse it
/// (keeping the cached key valid). Returns None if `content` isn't a
/// well-formed encrypted note.
pub fn extract_salt(content: &str) -> Option<[u8; SALT_LEN]> {
    let trimmed = content.trim_start_matches('\u{feff}').trim_end();
    let payload = trimmed.strip_prefix(MAGIC)?;
    let bytes = STANDARD_NO_PAD.decode(payload.trim()).ok()?;
    if bytes.len() < SALT_LEN {
        return None;
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[..SALT_LEN]);
    Some(salt)
}

/// Encrypt with a caller-supplied salt. The nonce is ALWAYS fresh
/// (reusing a nonce under the same key would be catastrophic for
/// AES-GCM). Reusing the salt across saves of the same note is safe —
/// salt only needs to be unique per password — and lets the key cache
/// hit, turning a per-save Argon2 run into a HashMap lookup.
fn encrypt_with_salt(
    plaintext: &str,
    password: &str,
    salt: &[u8; SALT_LEN],
) -> Result<String, String> {
    if password.is_empty() {
        return Err("password is empty".into());
    }
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);

    let key_bytes = derive_key(password, salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce_obj = Nonce::from_slice(&nonce);
    let ciphertext = cipher
        .encrypt(nonce_obj, plaintext.as_bytes())
        .map_err(|e| format!("aes-gcm encrypt: {e}"))?;

    let mut envelope = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    envelope.extend_from_slice(salt);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);

    let mut s = String::from(MAGIC);
    s.push_str(&STANDARD_NO_PAD.encode(&envelope));
    s.push('\n');
    Ok(s)
}

/// Encrypt `plaintext` with `password`, generating a fresh random salt.
/// Use for the first encryption of a note. Re-saves should go through
/// `encrypt_reusing_salt` to keep the key cache warm.
pub fn encrypt(plaintext: &str, password: &str) -> Result<String, String> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    encrypt_with_salt(plaintext, password, &salt)
}

/// Encrypt `plaintext`, reusing the salt from `prior` (the note's
/// current on-disk content) when it's a valid envelope. Falls back to
/// a fresh salt if `prior` isn't encrypted yet. This is the save path.
pub fn encrypt_reusing_salt(
    plaintext: &str,
    password: &str,
    prior: &str,
) -> Result<String, String> {
    match extract_salt(prior) {
        Some(salt) => encrypt_with_salt(plaintext, password, &salt),
        None => encrypt(plaintext, password),
    }
}

/// Decrypt the file `content` with `password`. Returns the plaintext if
/// the envelope is well-formed and the password is correct; otherwise
/// returns a human-readable error. Specifically uses a generic message
/// for any auth failure so we don't leak whether the password was wrong
/// vs. the ciphertext corrupted.
pub fn decrypt(content: &str, password: &str) -> Result<String, String> {
    let trimmed = content.trim_start_matches('\u{feff}').trim_end();
    let payload = trimmed
        .strip_prefix(MAGIC)
        .ok_or_else(|| "not an encrypted malt note".to_string())?;
    let bytes = STANDARD_NO_PAD
        .decode(payload.trim())
        .map_err(|e| format!("envelope base64: {e}"))?;
    if bytes.len() < SALT_LEN + NONCE_LEN + TAG_LEN {
        return Err("envelope too short".into());
    }
    let salt = &bytes[..SALT_LEN];
    let nonce = &bytes[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &bytes[SALT_LEN + NONCE_LEN..];

    let key_bytes = derive_key(password, salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| "wrong password or corrupted file".to_string())?;
    String::from_utf8(plaintext).map_err(|e| format!("plaintext utf-8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_simple() {
        let pt = "hello world\nwith multiple\n\nlines";
        let env = encrypt(pt, "correct horse battery staple").unwrap();
        assert!(is_encrypted(&env));
        let decoded = decrypt(&env, "correct horse battery staple").unwrap();
        assert_eq!(decoded, pt);
    }

    #[test]
    fn wrong_password_fails() {
        let env = encrypt("secret", "right").unwrap();
        assert!(decrypt(&env, "wrong").is_err());
    }

    #[test]
    fn nondeterministic_ciphertext() {
        let a = encrypt("same plaintext", "pw").unwrap();
        let b = encrypt("same plaintext", "pw").unwrap();
        assert_ne!(a, b, "fresh salt+nonce should make every encryption unique");
    }

    #[test]
    fn detects_non_envelope() {
        assert!(!is_encrypted("# regular markdown"));
        assert!(!is_encrypted("MALT-ENC-v1 but no colon"));
        // A note that merely MENTIONS the magic string is not an envelope —
        // short or space-broken tails must stay ordinary, openable notes.
        assert!(!is_encrypted("MALT-ENC-v1:abc"));
        assert!(!is_encrypted("MALT-ENC-v1: looks like this on disk"));
        // A real envelope still detects (full round-trip covered elsewhere).
        let env = encrypt("x", "pw").unwrap();
        assert!(is_encrypted(&env));
    }

    #[test]
    fn reuse_salt_keeps_salt_stable_but_varies_ciphertext() {
        let first = encrypt("v1 body", "pw").unwrap();
        let salt1 = extract_salt(&first).unwrap();
        // Re-save with reused salt — salt must match, but the full
        // envelope must differ (fresh nonce → different ciphertext).
        let second = encrypt_reusing_salt("v2 body", "pw", &first).unwrap();
        let salt2 = extract_salt(&second).unwrap();
        assert_eq!(salt1, salt2, "salt should be reused across re-saves");
        assert_ne!(first, second, "fresh nonce must change the envelope");
        assert_eq!(decrypt(&second, "pw").unwrap(), "v2 body");
    }

    #[test]
    fn reuse_salt_falls_back_for_plaintext_prior() {
        // First encryption of a previously-plaintext note: no prior
        // envelope to borrow a salt from, so it generates a fresh one.
        let env = encrypt_reusing_salt("body", "pw", "# was plaintext").unwrap();
        assert!(is_encrypted(&env));
        assert_eq!(decrypt(&env, "pw").unwrap(), "body");
    }

    #[test]
    fn cached_key_matches_uncached() {
        // Deriving twice with the same salt+password (second hits cache)
        // must yield identical keys — proven transitively by a decrypt
        // round-trip that spans a cache populate + hit.
        let salt = [7u8; SALT_LEN];
        let a = encrypt_with_salt("x", "pw", &salt).unwrap();
        let b = encrypt_with_salt("y", "pw", &salt).unwrap();
        assert_eq!(decrypt(&a, "pw").unwrap(), "x");
        assert_eq!(decrypt(&b, "pw").unwrap(), "y");
    }
}
