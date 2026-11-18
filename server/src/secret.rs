//! Sealing of secrets at rest (Filen session and password, tokens of the apps that are
//! pulled). AES-256-GCM with a key derived from `NRZBACKUP_SECRET` (umbrelOS: `APP_SEED`),
//! so nothing in the data folder alone reveals them.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    pub fn new(seed: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(b"nrzbackup-secrets-v1");
        h.update(seed);
        let key = h.finalize();
        SecretBox {
            cipher: Aes256Gcm::new_from_slice(&key).expect("32-byte key"),
        }
    }

    /// `v1:` + base64(nonce ‖ ciphertext ‖ tag). Empty input stays empty.
    pub fn seal(&self, plain: &str) -> String {
        if plain.is_empty() {
            return String::new();
        }
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
            .expect("encryption cannot fail");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        format!("v1:{}", B64.encode(out))
    }

    pub fn open(&self, sealed: &str) -> Option<String> {
        if sealed.is_empty() {
            return Some(String::new());
        }
        let raw = B64.decode(sealed.strip_prefix("v1:")?).ok()?;
        if raw.len() < 12 + 16 {
            return None;
        }
        let (nonce, ct) = raw.split_at(12);
        let plain = self.cipher.decrypt(Nonce::from_slice(nonce), ct).ok()?;
        String::from_utf8(plain).ok()
    }
}

/// Random token with prefix: `nrzbk_` + 40 hex chars.
pub fn new_token(prefix: &str) -> String {
    let mut b = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut b);
    format!("{prefix}{}", hex::encode(b))
}

/// Tokens are stored as SHA-256 only.
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// `nrzbk_3f9a…` for display.
pub fn token_hint(token: &str) -> String {
    let cut = token
        .char_indices()
        .nth(10)
        .map(|(i, _)| i)
        .unwrap_or(token.len());
    format!("{}…", &token[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let b = SecretBox::new(b"seed");
        let s = b.seal("geheim");
        assert!(s.starts_with("v1:"));
        assert_eq!(b.open(&s).as_deref(), Some("geheim"));
        assert_eq!(SecretBox::new(b"other").open(&s), None);
        assert_eq!(b.open("").as_deref(), Some(""));
    }
}
