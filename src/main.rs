mod app;
mod config;
mod error;
mod models;
mod openai;
mod storage;

use std::{process::Stdio, sync::Arc};

use app::{AppState, build_router, build_router_dev};
use config::AppConfig;
use error::AppResult;
use openai::OpenAiChatClient;
use storage::JsonStore;
use tokio::{net::TcpListener, process::Command};

#[tokio::main]
async fn main() -> AppResult<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let is_dev = std::env::args().any(|arg| arg == "dev");

    let config = AppConfig::from_env();
    let addr = config.socket_addr()?;
    let store = Arc::new(JsonStore::new(&config.data_dir).await?);
    let llm = Arc::new(OpenAiChatClient::new(config));
    let state = AppState { store, llm };

    let app = if is_dev {
        tracing::info!("starting Vite dev server...");
        Command::new("cmd.exe")
            .args(["/d", "/c", "cd frontend && npx vite --host 127.0.0.1 --port 5173"])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to start Vite; make sure Node.js is installed");
        build_router_dev(state)
    } else {
        build_router(state)
    };

    if is_dev {
        tracing::info!("WeiAgent listening on http://127.0.0.1:5173");
    } else {
        tracing::info!("WeiAgent listening on http://{addr}");
    }
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
