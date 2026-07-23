mod app;
mod config;
mod domain;
mod error;
mod fsutil;
mod llm;
mod storage;
mod tools;

use std::{io::ErrorKind, process::Stdio, sync::Arc};

use app::{AppState, build_router, build_router_dev};
use config::{AppConfig, log_level_filter};
use error::{AppError, AppResult};
use llm::registry::DefaultLlmRegistry;
use storage::{JsonStore, LlmLogger};
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
    let tool_defaults = store.read_tool_defaults().await?;
    if tool_defaults.evidence_storage.enabled
        && let Err(error) = tools::web_search::evidence::cleanup_evidence_store(
            &config.data_dir,
            &tool_defaults.evidence_storage,
        )
        .await
    {
        tracing::warn!(error = %error, "failed to clean evidence store during startup");
    }
    let llm_logger = Arc::new(LlmLogger::new(
        &config.data_dir,
        config.settings.llm_log_enabled,
    )?);
    let llm_registry = Arc::new(DefaultLlmRegistry::new(llm_logger.clone()));
    let approval_manager = Arc::new(tools::approval::ApprovalManager::new());
    let codex_runtime = Arc::new(tools::runtime::CodexRuntimeManager::new(&config.data_dir)?);
    let runtime_status = codex_runtime.initialize().await?;
    tracing::info!(
        available = runtime_status.available,
        source = ?runtime_status.source,
        version = ?runtime_status.detected_version,
        "Codex runtime status"
    );
    let terminal_manager = tools::terminal::TerminalManager::new(codex_runtime.clone());
    tokio::spawn(terminal_manager.clone().cleanup_loop());
    let tool_registry = Arc::new(tools::registry::ToolRegistry::with_builtin_tools());
    let state = AppState {
        store,
        llm_registry,
        llm_logger,
        terminal_manager,
        approval_manager,
        codex_runtime,
        tool_registry,
        run_manager: Arc::new(app::RunManager::new()),
    };
    let app = if is_dev {
        build_router_dev(state)
    } else {
        build_router(state)
    };
    let listener = TcpListener::bind(addr).await.map_err(|error| {
        if error.kind() == ErrorKind::AddrInUse {
            AppError::AddressInUse(addr)
        } else {
            AppError::Storage(error)
        }
    })?;
    let _vite_child = if is_dev {
        Some(start_vite_dev_server().await?)
    } else {
        None
    };

    tracing::info!(data_dir = %config.data_dir.display(), "WeiAgent data directory");
    tracing::info!(terminal_workspace_dir = %config.terminal_workspace_dir.display(), "WeiAgent terminal workspace directory");
    tracing::info!("WeiAgent listening on http://{addr}");
    if is_dev {
        tracing::info!("WeiAgent dev frontend listening on http://127.0.0.1:5173");
    }
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
