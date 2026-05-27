mod app;
mod config;
mod error;
mod models;
mod openai;
mod storage;

use std::{process::Stdio, sync::Arc};

use app::{AppState, build_router, build_router_dev};
use config::{AppConfig, log_level_filter};
use error::AppResult;
use openai::OpenAiChatClient;
use storage::JsonStore;
use tokio::{
    net::TcpListener,
    process::{Child, Command},
};

#[tokio::main]
async fn main() -> AppResult<()> {
    let is_dev = std::env::args().any(|arg| arg == "dev");
    let config = AppConfig::load(is_dev).await?;

    tracing_subscriber::fmt()
        .with_max_level(log_level_filter(&config.settings.log_level))
        .init();

    let addr = config.socket_addr()?;
    let store = Arc::new(JsonStore::new(&config.data_dir).await?);
    let llm = Arc::new(OpenAiChatClient::new());
    let state = AppState { store, llm };
    let _vite_child = if is_dev {
        Some(start_vite_dev_server().await?)
    } else {
        None
    };
    let app = if is_dev {
        build_router_dev(state)
    } else {
        build_router(state)
    };

    tracing::info!("WeiAgent listening on http://{addr}");
    if is_dev {
        tracing::info!("WeiAgent dev frontend listening on http://127.0.0.1:5173");
    }
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn start_vite_dev_server() -> AppResult<Child> {
    tracing::info!("starting Vite dev server...");
    Ok(Command::new(npm_command())
        .args(["run", "dev"])
        .current_dir("frontend")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?)
}

fn npm_command() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}
