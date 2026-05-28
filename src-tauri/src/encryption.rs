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

/// Cheap detector: a file is encrypted iff its first non-BOM bytes are
/// the magic prefix. Doesn't allocate; safe to call on huge files.
pub fn is_encrypted(content: &str) -> bool {
    content.trim_start_matches('\u{feff}').starts_with(MAGIC)
}

/// Derive a 32-byte AES key from `password` + `salt` using Argon2id with
/// library default parameters. Argon2id is the OWASP-recommended
/// password-hash function as of 2024.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], String> {
    let argon2 = Argon2::default();
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| format!("argon2: {e}"))?;
    Ok(key)
}

/// Encrypt `plaintext` with `password`. Returns the on-disk envelope
/// string (magic prefix + base64 payload). Each call generates a fresh
/// salt + nonce so two encryptions of the same text produce different
/// ciphertexts.
pub fn encrypt(plaintext: &str, password: &str) -> Result<String, String> {
    if password.is_empty() {
        return Err("password is empty".into());
    }
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);

    let key_bytes = derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce_obj = Nonce::from_slice(&nonce);
    let ciphertext = cipher
        .encrypt(nonce_obj, plaintext.as_bytes())
        .map_err(|e| format!("aes-gcm encrypt: {e}"))?;

    let mut envelope = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    envelope.extend_from_slice(&salt);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);

    let mut s = String::from(MAGIC);
    s.push_str(&STANDARD_NO_PAD.encode(&envelope));
    s.push('\n');
    Ok(s)
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
        assert!(is_encrypted("MALT-ENC-v1:abc"));
    }
}
