//! The secrets of Credentials, sealed with AES-256-GCM under `SANCHORIS_CREDENTIAL_KEY`, with
//! the ID of the User as associated data (`architecture.md`「秘密と環境変数」).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::db::DbError;

#[derive(Clone)]
pub struct SecretKey(Aes256Gcm);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

impl SecretKey {
    pub fn new(key: [u8; 32]) -> SecretKey {
        SecretKey(Aes256Gcm::new(&key.into()))
    }

    /// `SANCHORIS_CREDENTIAL_KEY`: 32 bytes in base64.
    pub fn from_env() -> Result<SecretKey, DbError> {
        let text = std::env::var("SANCHORIS_CREDENTIAL_KEY")
            .map_err(|_| DbError::MissingEnv("SANCHORIS_CREDENTIAL_KEY"))?;
        let bytes: [u8; 32] = STANDARD
            .decode(text.trim())
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                DbError::Invalid("SANCHORIS_CREDENTIAL_KEY must be 32 bytes in base64".into())
            })?;
        Ok(SecretKey::new(bytes))
    }

    /// The nonce and the ciphertext, in base64.
    pub fn seal(&self, user: &str, plain: &str) -> String {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).expect("the system has randomness");
        let sealed = self
            .0
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain.as_bytes(),
                    aad: user.as_bytes(),
                },
            )
            .expect("AES-GCM seals any message");
        let mut out = nonce.to_vec();
        out.extend(sealed);
        STANDARD.encode(out)
    }

    pub fn open(&self, user: &str, sealed: &str) -> Result<String, DbError> {
        let invalid = || DbError::Invalid("a sealed secret does not open".into());
        let bytes = STANDARD.decode(sealed).map_err(|_| invalid())?;
        if bytes.len() < 12 {
            return Err(invalid());
        }
        let (nonce, body) = bytes.split_at(12);
        let plain = self
            .0
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: user.as_bytes(),
                },
            )
            .map_err(|_| invalid())?;
        String::from_utf8(plain).map_err(|_| invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_opens_only_for_its_user() {
        let key = SecretKey::new([3u8; 32]);
        let sealed = key.seal("u1", "token");
        assert_eq!(key.open("u1", &sealed).unwrap(), "token");
        assert!(key.open("u2", &sealed).is_err());
        assert!(SecretKey::new([4u8; 32]).open("u1", &sealed).is_err());
    }
}
