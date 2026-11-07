use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use pgrustadmin_app_core::{EmailAddress, PasswordHash};
use pgrustadmin_postgres_driver::PgPool;
use pgrustadmin_postgres_driver::{create_session, find_user_for_login, revoke_session};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    password::{PasswordError, hash_password, verify_password},
    session::{SessionToken, session_user_id_from_headers, token_hash_from_headers},
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
    if let Some(token_hash) = token_hash_from_headers(&headers)
        && revoke_session(&pool, &token_hash).await.is_err()
    {
        return internal_error();
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn current_session(State(pool): State<PgPool>, headers: HeaderMap) -> Response {
    match session_user_id_from_headers(&pool, &headers).await {
        Ok(Some(user_id)) => (StatusCode::OK, Json(SessionResponse { user_id })).into_response(),
        Ok(None) => unauthorized(),
        Err(_) => internal_error(),
    }
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
    use std::env;

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header::AUTHORIZATION},
    };
    use pgrustadmin_postgres_driver::{
        connect_metadata_pool, create_session, run_metadata_migrations,
    };
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::auth::{
        password::hash_password,
        session::{SessionToken, token_hash_from_headers},
    };

    async fn send(
        app: &Router,
        method: Method,
        uri: &str,
        token: Option<&str>,
        payload: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        if payload.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let body = payload.map_or_else(Body::empty, |value| Body::from(value.to_string()));
        let response = app
            .clone()
            .oneshot(builder.body(body).expect("build test request"))
            .await
            .expect("auth router responds");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4096)
            .await
            .expect("read response body");
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("decode JSON response")
        };
        (status, json)
    }

    /// Covers the acceptance criteria of `specs/authentication.md` over HTTP.
    #[tokio::test]
    async fn login_sessions_and_logout_follow_the_authentication_contract() {
        let database_url = env::var("PGRUSTADMIN_TEST_DATABASE_URL")
            .expect("PGRUSTADMIN_TEST_DATABASE_URL is required for database tests");
        let pool = connect_metadata_pool(&database_url)
            .await
            .expect("connect to the isolated test metadata database");
        run_metadata_migrations(&pool)
            .await
            .expect("apply metadata migrations to the test database");

        let password = "correct horse battery";
        let hash = hash_password(password).expect("hash test password");
        let active_id = Uuid::new_v4();
        let inactive_id = Uuid::new_v4();
        let active_email = format!("auth-{active_id}@example.test");
        let inactive_email = format!("auth-{inactive_id}@example.test");
        for (id, email, active) in [
            (active_id, &active_email, true),
            (inactive_id, &inactive_email, false),
        ] {
            sqlx::query(
                "INSERT INTO users (id, email, password_hash, is_active) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(email)
            .bind(hash.as_encoded())
            .bind(active)
            .execute(&pool)
            .await
            .expect("insert test user");
        }
        let app = super::router(pool.clone());

        let (status, login) = send(
            &app,
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(serde_json::json!({
                "email": format!("  {}  ", active_email.to_uppercase()),
                "password": password,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "email is normalized before lookup");
        assert_eq!(login["token_type"], "Bearer");
        assert_eq!(login["user_id"], active_id.to_string());
        let token = login["access_token"]
            .as_str()
            .expect("access token")
            .to_owned();
        let stored: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sessions WHERE user_id = $1 AND token_hash = sha256($2::bytea)",
        )
        .bind(active_id)
        .bind(token.as_bytes())
        .fetch_one(&pool)
        .await
        .expect("count sessions");
        assert_eq!(stored, 1, "only the token hash is stored");
        let (status, session) = send(
            &app,
            Method::GET,
            "/api/v1/auth/session",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(session["user_id"], active_id.to_string());

        let wrong_password = send(
            &app,
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(serde_json::json!({ "email": active_email, "password": "wrong password!" })),
        )
        .await;
        let unknown_email = send(
            &app,
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(serde_json::json!({
                "email": format!("missing-{}@example.test", Uuid::new_v4()),
                "password": password,
            })),
        )
        .await;
        let inactive_user = send(
            &app,
            Method::POST,
            "/api/v1/auth/login",
            None,
            Some(serde_json::json!({ "email": inactive_email, "password": password })),
        )
        .await;
        let generic = (
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error": "invalid_credentials"}),
        );
        assert_eq!(wrong_password, generic);
        assert_eq!(
            unknown_email, generic,
            "does not reveal whether the email exists"
        );
        assert_eq!(inactive_user, generic);

        let expired = SessionToken::generate();
        create_session(&pool, active_id, &expired.token_hash())
            .await
            .expect("create session to expire");
        sqlx::query(
            "UPDATE sessions SET created_at = now() - interval '13 hours', \
             expires_at = now() - interval '1 hour' WHERE token_hash = $1",
        )
        .bind(expired.token_hash().as_slice())
        .execute(&pool)
        .await
        .expect("expire session");
        let unauthorized = (
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error": "unauthorized"}),
        );
        assert_eq!(
            send(
                &app,
                Method::GET,
                "/api/v1/auth/session",
                Some(expired.as_str()),
                None
            )
            .await,
            unauthorized
        );

        let deactivated = SessionToken::generate();
        create_session(&pool, inactive_id, &deactivated.token_hash())
            .await
            .expect("create session for inactive user");
        assert_eq!(
            send(
                &app,
                Method::GET,
                "/api/v1/auth/session",
                Some(deactivated.as_str()),
                None
            )
            .await,
            unauthorized,
            "sessions of inactive users are refused"
        );

        let (status, _) = send(
            &app,
            Method::POST,
            "/api/v1/auth/logout",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            send(
                &app,
                Method::GET,
                "/api/v1/auth/session",
                Some(&token),
                None
            )
            .await,
            unauthorized,
            "logout revokes the session on the server"
        );
        let (status, _) = send(
            &app,
            Method::POST,
            "/api/v1/auth/logout",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "logout is idempotent");
        assert_eq!(
            send(&app, Method::GET, "/api/v1/auth/session", None, None).await,
            unauthorized
        );
    }

    #[test]
    fn bearer_token_hash_accepts_case_insensitive_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("bEaReR opaque-token"),
        );

        assert_eq!(
            token_hash_from_headers(&headers),
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
            assert_eq!(token_hash_from_headers(&headers), None);
        }
    }
}
