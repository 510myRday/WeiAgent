mod app;
mod config;
mod domain;
mod error;
mod fsutil;
mod llm;
mod skills;
mod storage;
mod tools;

use std::{io::ErrorKind, process::Stdio, sync::Arc};

use app::{ApiSecurity, AppState, build_router, build_router_dev};
use config::{AppConfig, log_level_filter};
use error::{AppError, AppResult};
use llm::registry::DefaultLlmRegistry;
use storage::{LlmLogger, Store};
use tokio::{
    net::TcpListener,
    process::{Child, Command},
};

#[tokio::main]
async fn main() -> AppResult<()> {
    let is_dev = std::env::args().any(|arg| arg == "dev");
    let config = AppConfig::load(is_dev).await?;

    let log_filter = format!(
        "{},html5ever=error",
        log_level_filter(&config.settings.log_level)
    );
    tracing_subscriber::fmt().with_env_filter(log_filter).init();

    let addr = config.socket_addr()?;
    let api_security = Arc::new(ApiSecurity::new(addr, is_dev));
    let store = Arc::new(Store::new(&config.data_dir).await?);
    let database = store.database();
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
    let llm_logger = Arc::new(LlmLogger::with_database(
        database.clone(),
        config.settings.llm_log_enabled,
    )?);
    let llm_registry = Arc::new(DefaultLlmRegistry::new(llm_logger.clone()));
    let approval_manager = Arc::new(tools::approval::ApprovalManager::new());
    let environment_runtime = Arc::new(tools::runtime::EnvironmentRuntimeManager::new(
        &config.data_dir,
    )?);
    let environment_status = environment_runtime.initialize().await?;
    tracing::info!(
        components = environment_status.components.len(),
        "Local environment status"
    );
    let terminal_manager = tools::terminal::TerminalManager::new(environment_runtime.clone());
    tokio::spawn(terminal_manager.clone().cleanup_loop());
    let tool_registry = Arc::new(tools::registry::ToolRegistry::with_builtin_tools());
    let mcp_manager = Arc::new(tools::mcp::McpManager::new(
        store.clone(),
        environment_runtime.clone(),
    ));
    tokio::spawn(mcp_manager.clone().cleanup_loop());
    mcp_manager.load_cached().await?;
    let skill_manager = Arc::new(skills::SkillManager::new(store.clone()).await?);
    skill_manager.clone().start_monitor();
    let state = AppState {
        api_security: api_security.clone(),
        store,
        llm_registry,
        llm_logger: llm_logger.clone(),
        terminal_manager,
        approval_manager,
        environment_runtime,
        tool_registry,
        mcp_manager,
        skill_manager,
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
        Some(start_vite_dev_server(api_security.token()).await?)
    } else {
        None
    };

    tracing::info!(data_dir = %config.data_dir.display(), "WeiAgent data directory");
    tracing::info!(terminal_workspace_dir = %config.terminal_workspace_dir.display(), "WeiAgent terminal workspace directory");
    tracing::info!("WeiAgent listening on http://{addr}");
    if is_dev {
        tracing::info!("WeiAgent dev frontend listening on http://127.0.0.1:5173");
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    llm_logger.shutdown().await?;
    tracing::info!(
        failed_writes = llm_logger.failed_write_count(),
        "LLM logger stopped"
    );
    database.shutdown().await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

async fn start_vite_dev_server(api_token: &str) -> AppResult<Child> {
    tracing::info!("starting Vite dev server...");
    Ok(Command::new(npm_command())
        .args(["run", "dev"])
        .current_dir("frontend")
        .env("VITE_WEIAGENT_API_TOKEN", api_token)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?)
}

fn npm_command() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}
