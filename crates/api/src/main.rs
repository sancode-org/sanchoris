use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = required_env("SANCHORIS_BIND_ADDR")?.parse()?;
    let frontend_dir = PathBuf::from(required_env("SANCHORIS_FRONTEND_DIR")?);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("sanchoris: listening on http://{}", listener.local_addr()?);
    axum::serve(listener, sanchoris_api::router(&frontend_dir))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} must be set"))
}
