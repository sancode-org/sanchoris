use std::path::Path;

use axum::Router;
use axum::routing::get;
use tower_http::services::{ServeDir, ServeFile};

pub fn router(frontend_dir: &Path) -> Router {
    let index = ServeFile::new(frontend_dir.join("index.html"));
    Router::new()
        .route("/health", get(health))
        .fallback_service(ServeDir::new(frontend_dir).fallback(index))
}

async fn health() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let response = super::router(std::path::Path::new("."))
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }
}
