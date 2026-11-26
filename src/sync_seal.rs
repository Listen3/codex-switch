//! Password-sealed group documents stored in Redis.
//!
//! The password is the sync secret. It never enters the document, logs, or errors.

use anyhow::{Result, bail};
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hmac::Hmac;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const SEAL: &str = "chacha20poly1305-pbkdf2-sha256";
const ITERATIONS: u32 = 200_000;
const AAD: &[u8] = b"cs-sync-v1";

#[derive(Serialize, Deserialize)]
struct Envelope {
    schema: u32,
    seal: String,
    iters: u32,
    salt: String,
    nonce: String,
    ct: String,
}

pub fn seal(password: &str, plaintext: &[u8]) -> Result<String> {
    if password.is_empty() {
        bail!("sync password is empty");
    }
    let mut salt = [0_u8; 16];
    let mut nonce_bytes = [0_u8; 12];
    rand::rng().fill_bytes(&mut salt);
    rand::rng().fill_bytes(&mut nonce_bytes);
    let mut key_bytes = [0_u8; 32];
    pbkdf2_hmac_sha256(password.as_bytes(), &salt, ITERATIONS, &mut key_bytes);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("could not seal the group"))?;
    let envelope = Envelope {
        schema: 3,
        seal: SEAL.to_string(),
        iters: ITERATIONS,
        salt: b64(&salt),
        nonce: b64(&nonce_bytes),
        ct: b64(&ciphertext),
    };
    Ok(serde_json::to_string(&envelope)?)
}

pub fn open(password: &str, body: &str) -> Result<Vec<u8>> {
    let envelope: Envelope = serde_json::from_str(body).context_open()?;
    if envelope.schema != 3 || envelope.seal != SEAL {
        bail!("unrecognized sealed group");
    }
    if password.is_empty() {
        bail!("sync password is required to decrypt this group");
    }
    if envelope.iters == 0 || envelope.iters > 2_000_000 {
        bail!("unrecognized sealed group");
    }
    let salt = unb64(&envelope.salt)?;
    let nonce = unb64(&envelope.nonce)?;
    let ciphertext = unb64(&envelope.ct)?;
    if nonce.len() != 12 {
        bail!("unrecognized sealed group");
    }
    let mut key_bytes = [0_u8; 32];
    pbkdf2_hmac_sha256(password.as_bytes(), &salt, envelope.iters, &mut key_bytes);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("could not decrypt this group"))
}

fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8; 32]) {
    let mut block = [0_u8; 32];
    let mut u = hmac_sha256(password, &[salt, &1_u32.to_be_bytes()].concat());
    block.copy_from_slice(&u);
    for _ in 1..rounds {
        u = hmac_sha256(password, &u);
        for (left, right) in block.iter_mut().zip(u.iter()) {
            *left ^= right;
        }
    }
    *out = block;
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use hmac::{KeyInit, Mac};
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("hmac-sha256 accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| anyhow::anyhow!("unrecognized sealed group"))
}

trait OpenContext<T> {
    fn context_open(self) -> Result<T>;
}

impl<T> OpenContext<T> for Result<T, serde_json::Error> {
    fn context_open(self) -> Result<T> {
        self.map_err(|_| anyhow::anyhow!("unrecognized sealed group"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_roundtrip_and_wrong_password_fails() {
        let sealed = seal("correct horse", b"{\"schema\":2}").unwrap();
        assert!(sealed.contains("\"schema\":3"));
        assert!(!sealed.contains("{\"schema\":2}"));
        assert_eq!(open("correct horse", &sealed).unwrap(), b"{\"schema\":2}");
        assert!(open("wrong", &sealed).is_err());
        assert!(open("", &sealed).is_err());
    }
}
