use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use serde::Serialize;

pub fn router() -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/version", get(version))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct VersionResponse {
    version: &'static str,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

async fn health() -> impl IntoResponse {
    Json(HealthResponse { status: "ok" })
}

async fn version() -> impl IntoResponse {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse { error: "not_found" }),
    )
}

async fn method_not_allowed() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ErrorResponse {
            error: "method_not_allowed",
        }),
    )
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    use super::router;

    #[tokio::test]
    async fn health_and_version_are_public_and_return_json() {
        let app = router();
        let health = app
            .clone()
            .oneshot(
                Request::get("/api/v1/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), axum::http::StatusCode::OK);
        let health_body = to_bytes(health.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&health_body).unwrap(),
            serde_json::json!({"status": "ok"})
        );

        let version = app
            .oneshot(
                Request::get("/api/v1/version")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(version.status(), axum::http::StatusCode::OK);
        let version_body = to_bytes(version.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&version_body).unwrap(),
            serde_json::json!({"version": env!("CARGO_PKG_VERSION")})
        );
    }

    #[tokio::test]
    async fn unknown_paths_and_methods_return_stable_json_errors() {
        let app = router();
        for (request, expected_status, expected_error) in [
            (
                Request::get("/api/v1/missing")
                    .body(axum::body::Body::empty())
                    .unwrap(),
                axum::http::StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                Request::delete("/api/v1/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
                axum::http::StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
            ),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected_status);
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({"error": expected_error})
            );
        }
    }
}
