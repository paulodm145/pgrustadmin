use std::fmt;

use axum::http::{HeaderMap, header::AUTHORIZATION};
use pgrustadmin_postgres_driver::{PgPool, PostgresError, session_user_id};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Bearer secreto de uma sessão. O valor em texto é entregue ao cliente somente na criação.
pub struct SessionToken(String);

impl SessionToken {
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn hash(value: &str) -> [u8; 32] {
        Sha256::digest(value.as_bytes()).into()
    }

    #[must_use]
    pub fn token_hash(&self) -> [u8; 32] {
        Self::hash(&self.0)
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionToken([REDACTED])")
    }
}

pub(crate) fn token_hash_from_headers(headers: &HeaderMap) -> Option<[u8; 32]> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;

    if !scheme.eq_ignore_ascii_case("Bearer") || token.is_empty() || token.contains(' ') {
        return None;
    }

    Some(SessionToken::hash(token))
}

pub(crate) async fn session_user_id_from_headers(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<Option<Uuid>, PostgresError> {
    let Some(token_hash) = token_hash_from_headers(headers) else {
        return Ok(None);
    };

    session_user_id(pool, &token_hash).await
}

#[cfg(test)]
mod tests {
    use super::SessionToken;

    #[test]
    fn generates_independent_tokens_and_stable_hashes() {
        let first = SessionToken::generate();
        let second = SessionToken::generate();

        assert_ne!(first.as_str(), second.as_str());
        assert_eq!(first.token_hash(), SessionToken::hash(first.as_str()));
        assert_ne!(first.token_hash(), second.token_hash());
        assert_eq!(first.token_hash().len(), 32);
    }

    #[test]
    fn debug_output_does_not_reveal_token() {
        let token = SessionToken::generate();

        assert!(!format!("{token:?}").contains(token.as_str()));
        assert!(format!("{token:?}").contains("REDACTED"));
    }
}
