//! The tokens of the Thread channel (`architecture.md`「Thread channel」): the bootstrap token,
//! which one connection uses within 15 minutes, and the thread token, which the control plane
//! renews while the connection lasts. Both are signed with `THREAD_TOKEN_SIGNING_KEY`
//! (HMAC-SHA256) and name the User, the thread and the Workspace they are for.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::db::DbError;

/// How long a token is valid, in milliseconds.
pub const LIFETIME_MS: u64 = 15 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    Bootstrap,
    Thread,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub kind: TokenKind,
    pub user: String,
    pub thread: String,
    /// The `seq` of the Workspace.
    pub workspace: u64,
    /// Milliseconds since the Unix epoch.
    pub expires_at: u64,
    /// A random ID; a bootstrap token is spent by marking it.
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("the token is not valid")]
    Invalid,
    #[error("the token has expired")]
    Expired,
}

/// The key that signs and checks the tokens.
#[derive(Clone)]
pub struct TokenKey(Vec<u8>);

impl std::fmt::Debug for TokenKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenKey(..)")
    }
}

impl TokenKey {
    pub fn new(bytes: impl Into<Vec<u8>>) -> TokenKey {
        TokenKey(bytes.into())
    }

    /// `THREAD_TOKEN_SIGNING_KEY`, at least 32 bytes.
    pub fn from_env() -> Result<TokenKey, DbError> {
        let key = std::env::var("THREAD_TOKEN_SIGNING_KEY")
            .map_err(|_| DbError::MissingEnv("THREAD_TOKEN_SIGNING_KEY"))?;
        if key.len() < 32 {
            return Err(DbError::Invalid(
                "THREAD_TOKEN_SIGNING_KEY must have at least 32 bytes".into(),
            ));
        }
        Ok(TokenKey(key.into_bytes()))
    }

    fn mac(&self, payload: &str) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC takes any key");
        mac.update(payload.as_bytes());
        mac
    }

    /// Signs any text, for the local presigned URLs.
    pub fn sign(&self, payload: &str) -> String {
        URL_SAFE_NO_PAD.encode(self.mac(payload).finalize().into_bytes())
    }

    pub fn verify_signature(&self, payload: &str, signature: &str) -> bool {
        URL_SAFE_NO_PAD
            .decode(signature)
            .is_ok_and(|bytes| self.mac(payload).verify_slice(&bytes).is_ok())
    }

    pub fn issue(&self, claims: &Claims) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims are JSON"));
        let signature = self.sign(&payload);
        format!("{payload}.{signature}")
    }

    pub fn verify(&self, token: &str, now: u64) -> Result<Claims, TokenError> {
        let (payload, signature) = token.split_once('.').ok_or(TokenError::Invalid)?;
        if !self.verify_signature(payload, signature) {
            return Err(TokenError::Invalid);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| TokenError::Invalid)?;
        let claims: Claims = serde_json::from_slice(&bytes).map_err(|_| TokenError::Invalid)?;
        if claims.expires_at <= now {
            return Err(TokenError::Expired);
        }
        Ok(claims)
    }
}

/// A random ID of 128 bits, as 32 hexadecimal digits.
pub fn random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system has randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A token of `kind` for a Workspace of a thread, valid for [`LIFETIME_MS`] from `now`.
pub fn claims(kind: TokenKind, user: &str, thread: &str, workspace: u64, now: u64) -> Claims {
    Claims {
        kind,
        user: user.to_string(),
        thread: thread.to_string(),
        workspace,
        expires_at: now + LIFETIME_MS,
        id: random_id(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> TokenKey {
        TokenKey::new(vec![7u8; 32])
    }

    #[test]
    fn a_token_checks_with_its_key_until_it_expires() {
        let c = claims(TokenKind::Bootstrap, "u1", "th_1", 1, 1_000);
        let token = key().issue(&c);
        assert_eq!(key().verify(&token, 1_001), Ok(c.clone()));
        assert_eq!(key().verify(&token, c.expires_at), Err(TokenError::Expired));
        assert_eq!(
            TokenKey::new(vec![8u8; 32]).verify(&token, 1_001),
            Err(TokenError::Invalid)
        );
    }

    #[test]
    fn a_changed_token_does_not_check() {
        let token = key().issue(&claims(TokenKind::Thread, "u1", "th_1", 1, 1_000));
        let (payload, signature) = token.split_once('.').unwrap();
        let other = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&claims(TokenKind::Thread, "u2", "th_1", 1, 1_000)).unwrap(),
        );
        assert_eq!(
            key().verify(&format!("{other}.{signature}"), 1_001),
            Err(TokenError::Invalid)
        );
        assert_eq!(
            key().verify(&format!("{payload}x.{signature}"), 1_001),
            Err(TokenError::Invalid)
        );
        assert_eq!(key().verify("nodot", 1_001), Err(TokenError::Invalid));
    }
}
