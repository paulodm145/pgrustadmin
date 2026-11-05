use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pgrustadmin_app_core::{EmailAddress, PasswordHash};
use pgrustadmin_postgres_driver::PgPool;
use pgrustadmin_postgres_driver::{
    create_session, find_user_for_login, revoke_session, session_user_id,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    password::{PasswordError, hash_password, verify_password},
    session::SessionToken,
};

const SESSION_LIFETIME_SECONDS: u64 = 12 * 60 * 60;

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/session", get(current_session))
        .with_state(pool)
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: u64,
    user_id: Uuid,
}

#[derive(Serialize)]
struct SessionResponse {
    user_id: Uuid,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

async fn login(State(pool): State<PgPool>, Json(request): Json<LoginRequest>) -> Response {
    let Ok(email) = EmailAddress::new(&request.email) else {
        return invalid_credentials();
    };

    let account = match find_user_for_login(&pool, email.as_str()).await {
        Ok(account) => account,
        Err(_) => return internal_error(),
    };

    let (user_id, encoded_hash, is_active) = match account {
        Some((user_id, encoded_hash, is_active)) => (Some(user_id), Some(encoded_hash), is_active),
        None => (None, None, false),
    };

    let password = request.password;
    let verification = tokio::task::spawn_blocking(move || {
        if let Some(encoded_hash) = encoded_hash {
            let Ok(stored_hash) = PasswordHash::from_encoded(encoded_hash) else {
                return Ok(false);
            };
            verify_password(&password, &stored_hash)
        } else {
            hash_password(&password).map(|_| false)
        }
    })
    .await;

    let password_matches = match verification {
        Ok(Ok(matches)) => matches,
        Ok(Err(PasswordError::InvalidStoredHash)) => false,
        Ok(Err(_)) | Err(_) => return internal_error(),
    };

    let (Some(user_id), true, true) = (user_id, password_matches, is_active) else {
        return invalid_credentials();
    };

    let token = SessionToken::generate();
    if create_session(&pool, user_id, &token.token_hash())
        .await
        .is_err()
    {
        return internal_error();
    }

    (
        StatusCode::OK,
        Json(LoginResponse {
            access_token: token.as_str().to_owned(),
            token_type: "Bearer",
            expires_in: SESSION_LIFETIME_SECONDS,
            user_id,
        }),
    )
        .into_response()
}

async fn logout(State(pool): State<PgPool>, headers: HeaderMap) -> Response {
    if let Some(token_hash) = bearer_token_hash(&headers)
        && revoke_session(&pool, &token_hash).await.is_err()
    {
        return internal_error();
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn current_session(State(pool): State<PgPool>, headers: HeaderMap) -> Response {
    let Some(token_hash) = bearer_token_hash(&headers) else {
        return unauthorized();
    };

    match session_user_id(&pool, &token_hash).await {
        Ok(Some(user_id)) => (StatusCode::OK, Json(SessionResponse { user_id })).into_response(),
        Ok(None) => unauthorized(),
        Err(_) => internal_error(),
    }
}

fn bearer_token_hash(headers: &HeaderMap) -> Option<[u8; 32]> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;

    if !scheme.eq_ignore_ascii_case("Bearer") || token.is_empty() || token.contains(' ') {
        return None;
    }

    Some(SessionToken::hash(token))
}

fn invalid_credentials() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: "invalid_credentials",
        }),
    )
        .into_response()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: "unauthorized",
        }),
    )
        .into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "internal_error",
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    use super::bearer_token_hash;

    #[test]
    fn bearer_token_hash_accepts_case_insensitive_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("bEaReR opaque-token"),
        );

        assert_eq!(
            bearer_token_hash(&headers),
            Some(crate::auth::session::SessionToken::hash("opaque-token"))
        );
    }

    #[test]
    fn bearer_token_hash_rejects_missing_or_malformed_headers() {
        for value in ["", "Basic opaque-token", "Bearer", "Bearer token extra"] {
            let mut headers = HeaderMap::new();
            if !value.is_empty() {
                headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            }
            assert_eq!(bearer_token_hash(&headers), None);
        }
    }
}
