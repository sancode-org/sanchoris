//! The HTTP side of the control plane: GraphQL for the screens, the Thread channel's
//! WebSocket, the local routes that stand in for S3's presigned URLs, and the screens.

pub mod local;

use std::path::Path;
use std::sync::Arc;

use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt, StreamExt};
use sanchoris_graphql::{SanchorisSchema, Viewer};
use sanchoris_worker::channel::{self, Refused, Session};
use sanchoris_worker::local::{LocalConnections, LocalUrls};
use sanchoris_worker::ports::{Ctx, now};
use tower_http::services::{ServeDir, ServeFile};

/// The largest frame of the Thread channel (API Gateway takes messages up to 128 KB).
pub const MAX_FRAME: usize = 128 * 1024;

#[derive(Clone)]
pub struct App {
    pub ctx: Ctx,
    pub schema: SanchorisSchema,
    pub connections: LocalConnections,
    /// The User of every request of the local server.
    pub local_user: Option<String>,
}

pub fn router(frontend_dir: &Path, app: Option<Arc<App>>) -> Router {
    let index = ServeFile::new(frontend_dir.join("index.html"));
    let mut router = Router::new().route("/health", get(health));
    if let Some(app) = app {
        router = router.merge(
            Router::new()
                .route("/graphql", post(graphql))
                .route("/threads/channel", get(thread_channel))
                .route("/local/blobs/{*key}", get(blob))
                .route("/local/records", post(upload))
                .with_state(app),
        );
    }
    router.fallback_service(ServeDir::new(frontend_dir).fallback(index))
}

async fn health() -> &'static str {
    "ok"
}

async fn graphql(State(app): State<Arc<App>>, request: GraphQLRequest) -> GraphQLResponse {
    let mut request = request.into_inner();
    if let Some(user) = &app.local_user {
        request = request.data(Viewer(user.clone()));
    }
    app.schema.execute(request).await.into()
}

async fn thread_channel(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match channel::connect(&app.ctx, authorization, now()).await {
        Err(Refused::Unauthorized) => StatusCode::UNAUTHORIZED.into_response(),
        Err(Refused::Forbidden) => StatusCode::FORBIDDEN.into_response(),
        Ok(session) => ws
            .max_message_size(MAX_FRAME)
            .max_frame_size(MAX_FRAME)
            .on_upgrade(move |socket| serve(app, session, socket)),
    }
}

/// One connection of the Thread channel: frames of the agent go to [`channel::receive`] in
/// order, and the dispatcher's messages for the connection go out.
async fn serve(app: Arc<App>, session: Session, socket: WebSocket) {
    let id = sanchoris_core::token::random_id();
    let mut outgoing = app.connections.open(&id);
    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            Some(frame) = outgoing.recv() => {
                if sink.send(Message::Text(frame.into())).await.is_err() {
                    break;
                }
            }
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    match channel::receive(&app.ctx, &session, &id, text.as_str(), now()).await {
                        Ok(reply) => {
                            let mut failed = false;
                            for frame in reply.frames {
                                failed |= sink.send(Message::Text(frame.into())).await.is_err();
                            }
                            if reply.close || failed {
                                let _ = sink.close().await;
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("sanchoris: thread {}: {e}", session.thread);
                            let _ = sink
                                .send(Message::Close(Some(CloseFrame {
                                    code: 1011,
                                    reason: "internal error".into(),
                                })))
                                .await;
                            break;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    app.connections.close(&id);
    if let Err(e) = channel::disconnect(&app.ctx, &session, &id).await {
        eprintln!(
            "sanchoris: the disconnect of thread {}: {e}",
            session.thread
        );
    }
}

#[derive(serde::Deserialize)]
struct Signed {
    expires: u64,
    signature: String,
}

fn local_key(app: &App) -> &sanchoris_core::token::TokenKey {
    &app.ctx.tokens
}

async fn blob(
    State(app): State<Arc<App>>,
    axum::extract::Path(key): axum::extract::Path<String>,
    Query(signed): Query<Signed>,
) -> Response {
    let payload = LocalUrls::download_payload(&key, signed.expires);
    if signed.expires <= now() || !local_key(&app).verify_signature(&payload, &signed.signature) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match app.ctx.blobs.get(&key).await {
        Ok(Some(bytes)) => bytes.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The presigned POST of the thread records: the policy (`prefix`, `expires`, `signature`)
/// allows keys under its prefix until it expires.
async fn upload(State(app): State<Arc<App>>, mut form: Multipart) -> Response {
    let (mut prefix, mut expires, mut signature, mut key, mut file) =
        (None, None, None, None, None);
    while let Ok(Some(field)) = form.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            file = field.bytes().await.ok();
            continue;
        }
        let value = field.text().await.unwrap_or_default();
        match name.as_str() {
            "prefix" => prefix = Some(value),
            "expires" => expires = value.parse::<u64>().ok(),
            "signature" => signature = Some(value),
            "key" => key = Some(value),
            _ => {}
        }
    }
    let (Some(prefix), Some(expires), Some(signature), Some(key), Some(file)) =
        (prefix, expires, signature, key, file)
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let payload = LocalUrls::upload_payload(&prefix, expires);
    if expires <= now()
        || !key.starts_with(&prefix)
        || key.contains("..")
        || !local_key(&app).verify_signature(&payload, &signature)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match app.ctx.blobs.put(&key, file.to_vec()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let response = super::router(std::path::Path::new("."), None)
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }
}
