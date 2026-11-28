use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use sanchoris_api::local::{Settings, start};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = required_env("SANCHORIS_BIND_ADDR")?;
    let addr: SocketAddr = bind.parse()?;
    let frontend_dir = PathBuf::from(required_env("SANCHORIS_FRONTEND_DIR")?);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let app = start(Settings::from_env(&local.to_string())?).await?;
    eprintln!("sanchoris: listening on http://{local}");
    axum::serve(listener, sanchoris_api::router(&frontend_dir, Some(app)))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} must be set"))
}
