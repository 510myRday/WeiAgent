use std::{
    io::IsTerminal,
    net::IpAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command as TokioCommand},
    time::{sleep, timeout},
};
use uuid::Uuid;

use crate::{
    config::{api_token_path, resolve_data_dir},
    domain::ThinkingLevel,
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, Parser)]
#[command(name = "weiagent", version, about = "WeiAgent command line interface")]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "URL",
        env = "WEIAGENT_SERVER_URL",
        default_value = "http://127.0.0.1:18283"
    )]
    pub server: String,
    #[arg(long, global = true, value_name = "TOKEN", env = "WEIAGENT_API_TOKEN")]
    pub token: Option<String>,
    #[arg(long, global = true)]
    pub no_start: bool,
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
    #[arg(long, global = true)]
    pub quiet: bool,
    #[arg(long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub no_color: bool,
    #[arg(long, global = true, default_value_t = 30)]
    pub timeout: u64,
    #[arg(long, global = true, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

const IMPORT_POLL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Serve(ServerArgs),
    Dev(ServerArgs),
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    Cron {
        #[command(subcommand)]
        command: CronCommand,
    },
    Heartbeat {
        #[command(subcommand)]
        command: HeartbeatCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum CronCommand {
    List,
    Show { task_id: Uuid },
    Create(CronCreateArgs),
    Edit(CronEditArgs),
    Pause { task_id: Uuid },
    Resume { task_id: Uuid },
    Remove { task_id: Uuid },
    Run { task_id: Uuid },
    Executions { task_id: Uuid },
}

#[derive(Debug, Clone, Args)]
pub struct CronCreateArgs {
    #[arg(long)]
    pub agent: Uuid,
    #[arg(long)]
    pub schedule: String,
    pub prompt: Option<String>,
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
    #[arg(long)]
    pub conversation: Option<Uuid>,
    #[arg(long)]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct CronEditArgs {
    pub task_id: Uuid,
    #[arg(long)]
    pub schedule: Option<String>,
    pub prompt: Option<String>,
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
    #[arg(long)]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum HeartbeatCommand {
    Show {
        #[arg(long)]
        conversation: Uuid,
    },
    Set(HeartbeatSetArgs),
    Pause {
        #[arg(long)]
        conversation: Uuid,
    },
    Resume {
        #[arg(long)]
        conversation: Uuid,
    },
    Remove {
        #[arg(long)]
        conversation: Uuid,
    },
    Run {
        #[arg(long)]
        conversation: Uuid,
    },
}

#[derive(Debug, Clone, Args)]
pub struct HeartbeatSetArgs {
    #[arg(long)]
    pub conversation: Uuid,
    #[arg(long)]
    pub interval: String,
    pub prompt: Option<String>,
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ServerArgs {
    #[arg(long)]
    pub host: Option<String>,
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub foreground: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum AgentCommand {
    List,
    Show {
        agent_id: Uuid,
        #[arg(long)]
        full: bool,
    },
    Create(AgentCreateArgs),
    Edit(AgentEditArgs),
    Delete {
        agent_id: Uuid,
    },
    Run(AgentRunArgs),
    Profile {
        #[command(subcommand)]
        command: AgentProfileCommand,
    },
    Tools {
        #[command(subcommand)]
        command: AgentToolsCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: AgentMcpCommand,
    },
    Skill {
        #[command(subcommand)]
        command: AgentSkillCommand,
    },
}

#[derive(Debug, Clone, Args)]
pub struct AgentCreateArgs {
    #[arg(long)]
    pub full: bool,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, value_name = "UUID")]
    pub provider_id: Option<Uuid>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub avatar_icon: Option<String>,
    #[arg(long)]
    pub avatar_color: Option<String>,
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub profile_file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub tools_file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub mcp_file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub skills_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct AgentEditArgs {
    pub agent_id: Uuid,
    #[arg(long)]
    pub full: bool,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, value_name = "UUID")]
    pub provider_id: Option<Uuid>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub avatar_icon: Option<String>,
    #[arg(long)]
    pub avatar_color: Option<String>,
    #[arg(long)]
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub profile_file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub tools_file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub mcp_file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub skills_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum AgentProfileCommand {
    Show {
        agent_id: Uuid,
    },
    Edit {
        agent_id: Uuid,
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum AgentToolsCommand {
    Show {
        agent_id: Uuid,
    },
    Edit {
        agent_id: Uuid,
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum AgentMcpCommand {
    Show {
        agent_id: Uuid,
    },
    Edit {
        agent_id: Uuid,
        #[arg(long)]
        file: PathBuf,
    },
    Enable {
        agent_id: Uuid,
        server_id: Uuid,
    },
    Disable {
        agent_id: Uuid,
        server_id: Uuid,
    },
    ToolEnable {
        agent_id: Uuid,
        server_id: Uuid,
        tool_name: String,
    },
    ToolDisable {
        agent_id: Uuid,
        server_id: Uuid,
        tool_name: String,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum AgentSkillCommand {
    Show {
        agent_id: Uuid,
    },
    Edit {
        agent_id: Uuid,
        #[arg(long)]
        file: PathBuf,
    },
    Enable {
        agent_id: Uuid,
        skill_name: String,
    },
    Disable {
        agent_id: Uuid,
        skill_name: String,
    },
    GroupEnable {
        agent_id: Uuid,
        group_id: String,
    },
    GroupDisable {
        agent_id: Uuid,
        group_id: String,
    },
}

#[derive(Debug, Clone, Args)]
pub struct AgentRunArgs {
    pub agent_id: Uuid,
    pub prompt: Option<String>,
    #[arg(long)]
    pub conversation: Option<Uuid>,
    #[arg(long)]
    pub stream: bool,
    #[arg(long)]
    pub thinking_level: Option<ThinkingLevelArg>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ThinkingLevelArg {
    Off,
    Low,
    Medium,
    High,
    XHigh,
}

impl From<ThinkingLevelArg> for ThinkingLevel {
    fn from(value: ThinkingLevelArg) -> Self {
        match value {
            ThinkingLevelArg::Off => Self::Off,
            ThinkingLevelArg::Low => Self::Low,
            ThinkingLevelArg::Medium => Self::Medium,
            ThinkingLevelArg::High => Self::High,
            ThinkingLevelArg::XHigh => Self::XHigh,
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProviderCommand {
    List,
    Show { provider_id: Uuid },
    Create(ProviderCreateArgs),
    Edit(ProviderEditArgs),
    Delete { provider_id: Uuid },
    Test { provider_id: Uuid },
}

#[derive(Debug, Clone, Args)]
pub struct ProviderCreateArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub base_url: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub models: Vec<String>,
    #[arg(long, value_name = "TYPE")]
    pub provider_type: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub api_keys: Vec<String>,
    #[arg(long)]
    pub api_key_env: Option<String>,
    #[arg(long)]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct ProviderEditArgs {
    pub provider_id: Uuid,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub base_url: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub models: Option<Vec<String>>,
    #[arg(long, value_name = "TYPE")]
    pub provider_type: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub api_keys: Option<Vec<String>>,
    #[arg(long)]
    pub api_key_env: Option<String>,
    #[arg(long)]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ModelCommand {
    List {
        provider_id: Uuid,
    },
    Test {
        provider_id: Uuid,
        model_id: String,
        #[arg(long)]
        prompt: Option<String>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum SkillCommand {
    List,
    Show {
        skill_name: String,
    },
    Content {
        skill_name: String,
        #[arg(long)]
        path: Option<String>,
    },
    Run {
        skill_name: String,
        task: Option<String>,
        #[arg(long)]
        agent: Option<Uuid>,
    },
    Reload,
    Remove {
        skill_name: String,
    },
    Install {
        source: String,
        #[arg(long)]
        proxy_id: Option<Uuid>,
        #[arg(long)]
        group_id: Option<String>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum McpCommand {
    List,
    Show {
        server_id: Option<Uuid>,
    },
    Tools {
        server_id: Uuid,
    },
    Tool {
        #[command(subcommand)]
        command: McpToolCommand,
    },
    Add(McpFileArgs),
    Edit {
        server_id: Uuid,
        #[arg(long)]
        file: PathBuf,
    },
    Remove {
        server_id: Uuid,
    },
    Enable {
        server_id: Uuid,
    },
    Disable {
        server_id: Uuid,
    },
    Test {
        server_id: Uuid,
    },
    Reload {
        server_id: Option<Uuid>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum McpToolCommand {
    Show { server_id: Uuid, tool_name: String },
}

#[derive(Debug, Clone, Args)]
pub struct McpFileArgs {
    #[arg(long)]
    pub file: PathBuf,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProxyCommand {
    List,
    Show { proxy_id: Uuid },
    Add(ProxyCreateArgs),
    Edit(ProxyEditArgs),
    Remove { proxy_id: Uuid },
    Test { proxy_id: Uuid },
}

#[derive(Debug, Clone, Args)]
pub struct ProxyCreateArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long)]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct ProxyEditArgs {
    pub proxy_id: Uuid,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long)]
    pub enabled: Option<bool>,
    #[arg(long)]
    pub file: Option<PathBuf>,
}

pub fn is_server_command(cli: &Cli) -> bool {
    cli.command.is_none() || matches!(cli.command, Some(Command::Serve(_)) | Some(Command::Dev(_)))
}

impl Cli {
    pub fn is_server_command(&self) -> bool {
        is_server_command(self)
    }

    pub fn is_dev(&self) -> bool {
        matches!(self.command, Some(Command::Dev(_)))
    }

    pub fn apply_server_environment(&self) -> AppResult<()> {
        if let Some(path) = &self.data_dir {
            // Rust 2024 makes process-wide environment mutation explicit.
            unsafe {
                std::env::set_var("WEIAGENT_DATA_DIR", path);
            }
        }
        Ok(())
    }

    pub fn apply_settings(&self, settings: &mut crate::domain::AppSettings) -> AppResult<()> {
        let args = match &self.command {
            Some(Command::Serve(args)) | Some(Command::Dev(args)) => args,
            None => return Ok(()),
            _ => return Ok(()),
        };
        if let Some(host) = &args.host {
            settings.host = host.clone();
        }
        if let Some(port) = args.port {
            settings.port = port;
        }
        Ok(())
    }

    pub async fn run(self) -> AppResult<()> {
        match self.command.as_ref() {
            Some(Command::Agent { command }) => run_agent(&self, command.clone()).await,
            Some(Command::Provider { command }) => run_provider(&self, command.clone()).await,
            Some(Command::Model { command }) => run_model(&self, command.clone()).await,
            Some(Command::Skill { command }) => run_skill(&self, command.clone()).await,
            Some(Command::Mcp { command }) => run_mcp(&self, command.clone()).await,
            Some(Command::Proxy { command }) => run_proxy(&self, command.clone()).await,
            Some(Command::Cron { command }) => run_cron(&self, command.clone()).await,
            Some(Command::Heartbeat { command }) => run_heartbeat(&self, command.clone()).await,
            Some(Command::Serve(_) | Command::Dev(_)) | None => Ok(()),
        }
    }
}

#[derive(Debug, Clone)]
struct ApiClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
    stream_http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct AgentRunInfo {
    id: Uuid,
    status: String,
    error: Option<String>,
}

impl ApiClient {
    async fn connect(cli: &Cli) -> AppResult<(Self, Option<Child>)> {
        let base_url = cli.server.trim_end_matches('/').to_string();
        let timeout_value = Duration::from_secs(cli.timeout.max(1));
        let http = reqwest::Client::builder()
            .timeout(timeout_value)
            .build()
            .map_err(|error| {
                AppError::Process(format!("failed to create CLI HTTP client: {error}"))
            })?;
        let stream_http = reqwest::Client::builder().build().map_err(|error| {
            AppError::Process(format!("failed to create CLI stream client: {error}"))
        })?;
        let data_dir = cli.data_dir.clone().unwrap_or(resolve_data_dir(false)?);
        let token_path = api_token_path(&data_dir);
        let mut child = None;
        let mut token = valid_cli_token(cli, &http, &base_url).await?;

        if token.is_none() {
            if service_reachable(&http, &base_url).await {
                return Err(AppError::Process(format!(
                    "WeiAgent is already running at {base_url}, but the CLI could not find a matching API Token; pass --data-dir for that service or --token explicitly"
                )));
            }
            if cli.no_start {
                return Err(AppError::Process(format!(
                    "WeiAgent service is not available at {base_url}"
                )));
            }
            let executable = std::env::current_exe()?;
            let mut command = TokioCommand::new(executable);
            command
                .arg("serve")
                .arg("--foreground")
                .env("WEIAGENT_CLI_CHILD", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit());
            let (host, port) = local_server_bind(&base_url).ok_or_else(|| {
                AppError::Process(format!(
                    "WeiAgent service is not available at {base_url}; automatic startup is limited to loopback servers"
                ))
            })?;
            command
                .arg("--host")
                .arg(host)
                .arg("--port")
                .arg(port.to_string());
            if let Some(path) = &cli.data_dir {
                command.arg("--data-dir").arg(path);
            }
            child = Some(command.spawn().map_err(|error| {
                AppError::Process(format!("failed to start WeiAgent service: {error}"))
            })?);
            let deadline = Duration::from_secs(cli.timeout.max(5));
            let result = timeout(deadline, async {
                loop {
                    if let Ok(value) = tokio::fs::read_to_string(&token_path).await {
                        let value = value.trim().to_string();
                        if !value.is_empty() && service_ready(&http, &base_url, Some(&value)).await
                        {
                            break value;
                        }
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            })
            .await;
            token = Some(result.map_err(|_| {
                AppError::Process(format!(
                    "timed out waiting for WeiAgent service at {base_url}"
                ))
            })?);
        }

        let token = token.ok_or_else(|| {
            AppError::Process(
                "API Token was not found; pass --token or start WeiAgent first".to_string(),
            )
        })?;
        Ok((
            Self {
                base_url,
                token,
                http,
                stream_http,
            },
            child,
        ))
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> AppResult<T> {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AppError::Process(format!("request failed: {error}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| AppError::Process(format!("failed to read response: {error}")))?;
        if !status.is_success() {
            let detail = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| text.trim().to_string());
            return Err(AppError::Process(format!("HTTP {status}: {detail}")));
        }
        serde_json::from_str(&text).map_err(AppError::from)
    }

    async fn request_empty(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> AppResult<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AppError::Process(format!("request failed: {error}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::Process(format!("HTTP {status}: {text}")));
        }
        let text = response
            .text()
            .await
            .map_err(|error| AppError::Process(format!("failed to read response: {error}")))?;
        if text.trim().is_empty() {
            return Ok(json!({"ok": true}));
        }
        serde_json::from_str(&text).map_err(AppError::from)
    }

    async fn stream_run(
        &self,
        run_id: Uuid,
        stream_output: bool,
        format: OutputFormat,
        quiet: bool,
    ) -> AppResult<()> {
        let response = self
            .stream_http
            .get(format!("{}/api/runs/{run_id}/events", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|error| AppError::Process(format!("run event request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(AppError::Process(format!(
                "HTTP {} while opening run events",
                response.status()
            )));
        }
        let mut chunks = response.bytes_stream();
        let mut pending = String::new();
        let mut event_name = String::new();
        let mut event_data = String::new();
        let mut final_text = String::new();
        let mut terminal_status = "completed";
        while let Some(chunk) = futures_util::StreamExt::next(&mut chunks).await {
            let chunk = chunk.map_err(|error| {
                AppError::Process(format!("failed to read run events: {error}"))
            })?;
            pending.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(newline) = pending.find('\n') {
                let line = pending[..newline].trim_end_matches('\r').to_string();
                pending.drain(..=newline);
                if let Some(value) = line.strip_prefix("event:") {
                    event_name = value.trim().to_string();
                }
                if let Some(value) = line.strip_prefix("data:") {
                    event_data.push_str(value.trim());
                }
                if line.is_empty() {
                    if !event_data.is_empty() {
                        if event_name == "delta" {
                            let text = serde_json::from_str::<Value>(&event_data)
                                .ok()
                                .and_then(|value| {
                                    value
                                        .get("text")
                                        .and_then(Value::as_str)
                                        .map(str::to_string)
                                })
                                .unwrap_or_default();
                            final_text.push_str(&text);
                            if stream_output && !quiet {
                                match format {
                                    OutputFormat::Human => print!("{text}"),
                                    OutputFormat::Json => {
                                        println!("{}", json!({"event": "delta", "text": text}))
                                    }
                                }
                            }
                        } else if event_name == "assistant" {
                            if let Ok(value) = serde_json::from_str::<Value>(&event_data) {
                                if let Some(content) = value.get("content").and_then(Value::as_str)
                                {
                                    final_text = content.to_string();
                                }
                            }
                        } else if event_name == "error" {
                            return Err(AppError::Process(event_data.clone()));
                        } else if stream_output && !quiet && matches!(format, OutputFormat::Json) {
                            let data = serde_json::from_str::<Value>(&event_data)
                                .unwrap_or_else(|_| Value::String(event_data.clone()));
                            let line = json!({"event": event_name, "data": data});
                            println!("{}", serde_json::to_string(&line)?);
                        }
                        if event_name == "done" || event_name == "cancelled" {
                            if event_name == "cancelled" {
                                terminal_status = "cancelled";
                            }
                            break;
                        }
                    }
                    event_name.clear();
                    event_data.clear();
                }
            }
        }
        if !quiet && stream_output && matches!(format, OutputFormat::Human) {
            println!();
        }
        if !quiet && !stream_output && matches!(format, OutputFormat::Human) {
            println!("{final_text}");
        }
        if !quiet && matches!(format, OutputFormat::Json) && !stream_output {
            println!(
                "{}",
                json!({"run_id": run_id, "content": final_text, "status": terminal_status})
            );
        }
        Ok(())
    }
}

async fn valid_cli_token(
    cli: &Cli,
    http: &reqwest::Client,
    base_url: &str,
) -> AppResult<Option<String>> {
    if let Some(token) = cli
        .token
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return Ok(service_ready(http, base_url, Some(token))
            .await
            .then(|| token.to_string()));
    }

    for data_dir in cli_data_dir_candidates(cli)? {
        let Ok(value) = std::fs::read_to_string(api_token_path(&data_dir)) else {
            continue;
        };
        let token = value.trim();
        if !token.is_empty() && service_ready(http, base_url, Some(token)).await {
            return Ok(Some(token.to_string()));
        }
    }
    Ok(None)
}

fn local_server_bind(base_url: &str) -> Option<(String, u16)> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .ok()
            .is_some_and(|address| address.is_loopback());
    if !is_loopback {
        return None;
    }
    Some((host.to_string(), url.port_or_known_default()?))
}

fn cli_data_dir_candidates(cli: &Cli) -> AppResult<Vec<PathBuf>> {
    if let Some(data_dir) = &cli.data_dir {
        return Ok(vec![data_dir.clone()]);
    }

    let mut candidates = Vec::new();
    push_unique_path(&mut candidates, resolve_data_dir(false)?);
    push_unique_path(&mut candidates, resolve_data_dir(true)?);
    if let Ok(current_dir) = std::env::current_dir() {
        push_ancestor_data_dirs(&mut candidates, &current_dir);
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        push_ancestor_data_dirs(&mut candidates, directory);
    }
    Ok(candidates)
}

fn push_ancestor_data_dirs(candidates: &mut Vec<PathBuf>, start: &Path) {
    for ancestor in start.ancestors().take(8) {
        push_unique_path(candidates, ancestor.join("data"));
    }
}

fn push_unique_path(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.iter().any(|candidate| candidate == &path) {
        candidates.push(path);
    }
}

async fn service_ready(http: &reqwest::Client, base_url: &str, token: Option<&str>) -> bool {
    let Some(token) = token else {
        return false;
    };
    http.get(format!("{base_url}/api/settings"))
        .bearer_auth(token)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

async fn service_reachable(http: &reqwest::Client, base_url: &str) -> bool {
    http.get(format!("{base_url}/api/settings"))
        .send()
        .await
        .is_ok()
}

async fn read_prompt(prompt: Option<String>) -> AppResult<String> {
    if let Some(prompt) = prompt {
        return Ok(prompt);
    }
    if std::io::stdin().is_terminal() {
        return Err(AppError::BadRequest(
            "a prompt argument or stdin input is required".to_string(),
        ));
    }
    let mut value = String::new();
    tokio::io::stdin().read_to_string(&mut value).await?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(AppError::BadRequest("prompt input is empty".to_string()));
    }
    Ok(value)
}

async fn file_json(path: &Path) -> AppResult<Value> {
    let content = tokio::fs::read_to_string(path).await?;
    Ok(serde_json::from_str(&content)?)
}

fn set_if_present(object: &mut Value, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        object[key] = value;
    }
}

fn print_value(cli: &Cli, value: &impl Serialize) -> AppResult<()> {
    if cli.quiet {
        return Ok(());
    }
    if matches!(cli.format, OutputFormat::Json) {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

fn object_field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

fn project_array(value: Value, project: impl Fn(&Value) -> Value) -> Value {
    Value::Array(
        value
            .as_array()
            .map(|items| items.iter().map(project).collect())
            .unwrap_or_default(),
    )
}

fn summarize_agents(value: Value) -> Value {
    project_array(value, |item| {
        json!({
            "id": object_field(item, "id"),
            "name": object_field(item, "name"),
            "model_provider_id": object_field(item, "model_provider_id"),
            "model": object_field(item, "model_id"),
            "builtin": object_field(item, "is_builtin"),
        })
    })
}

fn summarize_providers(value: Value) -> Value {
    project_array(value, |item| {
        json!({
            "id": object_field(item, "id"),
            "name": object_field(item, "name"),
            "type": object_field(item, "provider_type"),
            "model_count": item
                .get("models")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default(),
            "has_api_key": object_field(item, "has_api_key"),
            "proxy_configured": item.get("proxy_url").is_some_and(|value| !value.is_null()),
        })
    })
}

fn summarize_models(value: Value) -> Value {
    Value::Array(
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|model| json!({"id": model, "name": model}))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn summarize_skills(value: Value) -> Value {
    let skills = value
        .get("defaults")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    json!({
                        "name": object_field(item, "name"),
                        "version": object_field(item, "version"),
                        "group": object_field(item, "group"),
                        "group_id": object_field(item, "group_id"),
                        "enabled": object_field(item, "enabled"),
                        "builtin": object_field(item, "is_builtin"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let groups = value
        .get("groups")
        .cloned()
        .map(|groups| {
            project_array(groups, |item| {
                json!({
                    "id": object_field(item, "id"),
                    "name": object_field(item, "name"),
                    "skill_count": object_field(item, "skill_count"),
                    "enabled": object_field(item, "enabled"),
                    "builtin": object_field(item, "is_builtin"),
                })
            })
        })
        .unwrap_or_else(|| Value::Array(Vec::new()));
    json!({"skills": skills, "groups": groups})
}

fn summarize_mcp_servers(value: Value) -> Value {
    project_array(value, |item| {
        json!({
            "id": object_field(item, "id"),
            "name": object_field(item, "name"),
            "enabled": object_field(item, "enabled"),
            "status": object_field(item, "status"),
            "provider": object_field(item, "provider"),
            "tool_count": item
                .get("tools")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default(),
        })
    })
}

fn summarize_mcp_server(value: Value) -> Value {
    let mut server = value.as_object().cloned().unwrap_or_default();
    server.remove("tools");
    Value::Object(server)
}

fn summarize_mcp_tools(value: &Value) -> Value {
    project_array(
        value
            .get("tools")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
        |item| {
            json!({
                "tool_id": object_field(item, "tool_id"),
                "name": object_field(item, "name"),
                "qualified_name": object_field(item, "qualified_name"),
                "title": object_field(item, "title"),
                "enabled": object_field(item, "enabled"),
            })
        },
    )
}

fn find_mcp_tool(value: &Value, tool_name: &str) -> Option<Value> {
    value
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| {
            tools.iter().find(|tool| {
                tool.get("name").and_then(Value::as_str) == Some(tool_name)
                    || tool.get("qualified_name").and_then(Value::as_str) == Some(tool_name)
            })
        })
        .cloned()
}

fn summarize_proxies(value: Value) -> Value {
    project_array(value, |item| {
        json!({
            "id": object_field(item, "id"),
            "name": object_field(item, "name"),
            "protocol": object_field(item, "protocol"),
            "enabled": object_field(item, "enabled"),
            "status": object_field(item, "status"),
            "latency_ms": object_field(item, "latency_ms"),
        })
    })
}

async fn run_agent(cli: &Cli, command: AgentCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        AgentCommand::List => print_value(
            cli,
            &summarize_agents(
                client
                    .request::<Value>(reqwest::Method::GET, "/api/agents", None)
                    .await?,
            ),
        ),
        AgentCommand::Show { agent_id, full } => {
            let value = if full {
                read_agent_full(&client, agent_id).await?
            } else {
                client
                    .request::<Value>(
                        reqwest::Method::GET,
                        &format!("/api/agents/{agent_id}"),
                        None,
                    )
                    .await?
            };
            print_value(cli, &value)
        }
        AgentCommand::Create(args) => {
            let full = args.full;
            let mut document = if let Some(path) = args.file.as_deref() {
                file_json(&path).await?
            } else {
                json!({})
            };
            let mut body = agent_document_section(&document);
            set_if_present(&mut body, "name", args.name.map(Value::String));
            set_if_present(
                &mut body,
                "description",
                args.description.map(Value::String),
            );
            set_if_present(
                &mut body,
                "model_provider_id",
                args.provider_id.map(|v| Value::String(v.to_string())),
            );
            set_if_present(&mut body, "model_id", args.model.map(Value::String));
            set_if_present(
                &mut body,
                "avatar_icon",
                args.avatar_icon.map(Value::String),
            );
            set_if_present(
                &mut body,
                "avatar_color",
                args.avatar_color.map(Value::String),
            );
            document["agent"] = body;
            let created = client
                .request::<Value>(
                    reqwest::Method::POST,
                    "/api/agents",
                    Some(document["agent"].clone()),
                )
                .await?;
            let agent_id = value_uuid(&created, "id")?;
            if let Err(error) = apply_agent_subconfigs(
                &client,
                agent_id,
                &document,
                args.profile_file.as_deref(),
                args.tools_file.as_deref(),
                args.mcp_file.as_deref(),
                args.skills_file.as_deref(),
            )
            .await
            {
                let _ = client
                    .request_empty(
                        reqwest::Method::DELETE,
                        &format!("/api/agents/{agent_id}"),
                        None,
                    )
                    .await;
                return Err(error);
            }
            let mut changed = vec!["agent"];
            if document.get("workspace").is_some() || args.profile_file.is_some() {
                changed.push("profile");
            }
            if document.get("tools").is_some() || args.tools_file.is_some() {
                changed.push("tools");
            }
            if document.get("mcp").is_some() || args.mcp_file.is_some() {
                changed.push("mcp");
            }
            if document.get("skills").is_some() || args.skills_file.is_some() {
                changed.push("skills");
            }
            if full {
                print_value(cli, &read_agent_full(&client, agent_id).await?)
            } else {
                print_value(
                    cli,
                    &json!({
                        "ok": true,
                        "agent_id": agent_id,
                        "changed": changed,
                    }),
                )
            }
        }
        AgentCommand::Edit(args) => {
            let id = args.agent_id;
            let full = args.full;
            let document = if let Some(path) = args.file.as_deref() {
                file_json(&path).await?
            } else {
                json!({})
            };
            let mut changed = Vec::new();
            let mut body = agent_document_section(&document);
            set_if_present(&mut body, "name", args.name.map(Value::String));
            set_if_present(
                &mut body,
                "description",
                args.description.map(Value::String),
            );
            set_if_present(
                &mut body,
                "model_provider_id",
                args.provider_id.map(|v| Value::String(v.to_string())),
            );
            set_if_present(&mut body, "model_id", args.model.map(Value::String));
            set_if_present(
                &mut body,
                "avatar_icon",
                args.avatar_icon.map(Value::String),
            );
            set_if_present(
                &mut body,
                "avatar_color",
                args.avatar_color.map(Value::String),
            );
            let has_basic = body.as_object().is_some_and(|object| !object.is_empty());
            if has_basic {
                changed.push("agent");
                client
                    .request::<Value>(
                        reqwest::Method::PUT,
                        &format!("/api/agents/{id}"),
                        Some(body),
                    )
                    .await?;
            }
            if document.get("workspace").is_some() || args.profile_file.is_some() {
                changed.push("profile");
            }
            if document.get("tools").is_some() || args.tools_file.is_some() {
                changed.push("tools");
            }
            if document.get("mcp").is_some() || args.mcp_file.is_some() {
                changed.push("mcp");
            }
            if document.get("skills").is_some() || args.skills_file.is_some() {
                changed.push("skills");
            }
            apply_agent_subconfigs(
                &client,
                id,
                &document,
                args.profile_file.as_deref(),
                args.tools_file.as_deref(),
                args.mcp_file.as_deref(),
                args.skills_file.as_deref(),
            )
            .await?;
            if full {
                print_value(cli, &read_agent_full(&client, id).await?)
            } else {
                print_value(
                    cli,
                    &json!({
                        "ok": true,
                        "agent_id": id,
                        "changed": changed,
                    }),
                )
            }
        }
        AgentCommand::Delete { agent_id } => print_value(
            cli,
            &client
                .request_empty(
                    reqwest::Method::DELETE,
                    &format!("/api/agents/{agent_id}"),
                    None,
                )
                .await?,
        ),
        AgentCommand::Run(args) => run_agent_task(cli, client, args).await,
        AgentCommand::Profile { command } => run_agent_profile(cli, client, command).await,
        AgentCommand::Tools { command } => run_agent_tools(cli, client, command).await,
        AgentCommand::Mcp { command } => run_agent_mcp(cli, client, command).await,
        AgentCommand::Skill { command } => run_agent_skill(cli, client, command).await,
    }
}

fn value_uuid(value: &Value, key: &str) -> AppResult<Uuid> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Process(format!("response did not include {key}")))?
        .parse()
        .map_err(|_| AppError::Process(format!("response included an invalid {key}")))
}

fn agent_document_section(document: &Value) -> Value {
    document
        .get("agent")
        .cloned()
        .unwrap_or_else(|| document.clone())
}

fn workspace_document_section(document: &Value) -> Option<Value> {
    document.get("workspace").cloned().or_else(|| {
        document
            .get("profile")
            .and_then(|value| value.get("workspace"))
            .cloned()
    })
}

async fn read_agent_full(client: &ApiClient, agent_id: Uuid) -> AppResult<Value> {
    let agent_path = format!("/api/agents/{agent_id}");
    let workspace_path = format!("/api/agents/{agent_id}/workspace");
    let tools_path = format!("/api/agents/{agent_id}/tools-config");
    let mcp_path = format!("/api/agents/{agent_id}/mcp-config");
    let skills_path = format!("/api/agents/{agent_id}/skills-config");
    let (agent, workspace, tools, mcp, skills) = tokio::join!(
        client.request::<Value>(reqwest::Method::GET, &agent_path, None),
        client.request::<Value>(reqwest::Method::GET, &workspace_path, None),
        client.request::<Value>(reqwest::Method::GET, &tools_path, None),
        client.request::<Value>(reqwest::Method::GET, &mcp_path, None),
        client.request::<Value>(reqwest::Method::GET, &skills_path, None),
    );
    Ok(json!({
        "agent": agent?,
        "workspace": workspace?,
        "tools": tools?,
        "mcp": mcp?,
        "skills": skills?,
    }))
}

async fn read_optional_config(path: Option<&Path>) -> AppResult<Option<Value>> {
    match path {
        Some(path) => Ok(Some(file_json(path).await?)),
        None => Ok(None),
    }
}

fn profile_request(agent: Value, workspace: Value) -> Value {
    json!({"agent": agent, "workspace": workspace})
}

async fn apply_agent_subconfigs(
    client: &ApiClient,
    agent_id: Uuid,
    document: &Value,
    profile_file: Option<&Path>,
    tools_file: Option<&Path>,
    mcp_file: Option<&Path>,
    skills_file: Option<&Path>,
) -> AppResult<()> {
    let workspace = match read_optional_config(profile_file).await? {
        Some(value) => Some(value.get("workspace").cloned().unwrap_or(value)),
        None => workspace_document_section(document),
    };
    if let Some(workspace) = workspace {
        client
            .request::<Value>(
                reqwest::Method::PUT,
                &format!("/api/agents/{agent_id}/profile"),
                Some(profile_request(json!({}), workspace)),
            )
            .await?;
    }

    let tools = match read_optional_config(tools_file).await? {
        Some(value) => Some(value),
        None => document.get("tools").cloned(),
    };
    if let Some(tools) = tools {
        client
            .request::<Value>(
                reqwest::Method::PUT,
                &format!("/api/agents/{agent_id}/tools-config"),
                Some(tools),
            )
            .await?;
    }

    let mcp = match read_optional_config(mcp_file).await? {
        Some(value) => Some(value),
        None => document.get("mcp").cloned(),
    };
    if let Some(mcp) = mcp {
        client
            .request::<Value>(
                reqwest::Method::PUT,
                &format!("/api/agents/{agent_id}/mcp-config"),
                Some(mcp),
            )
            .await?;
    }

    let skills = match read_optional_config(skills_file).await? {
        Some(value) => Some(value),
        None => document.get("skills").cloned(),
    };
    if let Some(skills) = skills {
        client
            .request::<Value>(
                reqwest::Method::PUT,
                &format!("/api/agents/{agent_id}/skills-config"),
                Some(skills),
            )
            .await?;
    }
    Ok(())
}

async fn run_agent_profile(
    cli: &Cli,
    client: ApiClient,
    command: AgentProfileCommand,
) -> AppResult<()> {
    match command {
        AgentProfileCommand::Show { agent_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/agents/{agent_id}/workspace"),
                    None,
                )
                .await?,
        ),
        AgentProfileCommand::Edit { agent_id, file } => {
            let value = file_json(&file).await?;
            let workspace = value.get("workspace").cloned().unwrap_or(value);
            print_value(
                cli,
                &client
                    .request::<Value>(
                        reqwest::Method::PUT,
                        &format!("/api/agents/{agent_id}/profile"),
                        Some(profile_request(json!({}), workspace)),
                    )
                    .await?,
            )
        }
    }
}

async fn run_agent_tools(
    cli: &Cli,
    client: ApiClient,
    command: AgentToolsCommand,
) -> AppResult<()> {
    match command {
        AgentToolsCommand::Show { agent_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/agents/{agent_id}/tools-config"),
                    None,
                )
                .await?,
        ),
        AgentToolsCommand::Edit { agent_id, file } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::PUT,
                    &format!("/api/agents/{agent_id}/tools-config"),
                    Some(file_json(&file).await?),
                )
                .await?,
        ),
    }
}

async fn run_agent_mcp(cli: &Cli, client: ApiClient, command: AgentMcpCommand) -> AppResult<()> {
    match command {
        AgentMcpCommand::Show { agent_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/agents/{agent_id}/mcp-config"),
                    None,
                )
                .await?,
        ),
        AgentMcpCommand::Edit { agent_id, file } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::PUT,
                    &format!("/api/agents/{agent_id}/mcp-config"),
                    Some(file_json(&file).await?),
                )
                .await?,
        ),
        AgentMcpCommand::Enable {
            agent_id,
            server_id,
        } => print_value(
            cli,
            &update_agent_mcp_server(&client, agent_id, server_id, true).await?,
        ),
        AgentMcpCommand::Disable {
            agent_id,
            server_id,
        } => print_value(
            cli,
            &update_agent_mcp_server(&client, agent_id, server_id, false).await?,
        ),
        AgentMcpCommand::ToolEnable {
            agent_id,
            server_id,
            tool_name,
        } => print_value(
            cli,
            &update_agent_mcp_tool(&client, agent_id, server_id, &tool_name, true).await?,
        ),
        AgentMcpCommand::ToolDisable {
            agent_id,
            server_id,
            tool_name,
        } => print_value(
            cli,
            &update_agent_mcp_tool(&client, agent_id, server_id, &tool_name, false).await?,
        ),
    }
}

async fn read_agent_override_items(
    client: &ApiClient,
    agent_id: Uuid,
    path: &str,
    key: &str,
) -> AppResult<Vec<Value>> {
    let settings = client
        .request::<Value>(
            reqwest::Method::GET,
            &format!("/api/agents/{agent_id}/{path}"),
            None,
        )
        .await?;
    Ok(settings
        .get("overrides")
        .and_then(|value| value.get(key))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn replace_or_append_override(
    items: &mut Vec<Value>,
    predicate: impl Fn(&Value) -> bool,
    value: Value,
) {
    if let Some(existing) = items.iter_mut().find(|item| predicate(item)) {
        *existing = value;
    } else {
        items.push(value);
    }
}

async fn update_agent_mcp_server(
    client: &ApiClient,
    agent_id: Uuid,
    server_id: Uuid,
    enabled: bool,
) -> AppResult<Value> {
    let mut servers = read_agent_override_items(client, agent_id, "mcp-config", "servers").await?;
    let tools = read_agent_override_items(client, agent_id, "mcp-config", "tools").await?;
    let server_key = server_id.to_string();
    replace_or_append_override(
        &mut servers,
        |item| item.get("server_id").and_then(Value::as_str) == Some(server_key.as_str()),
        json!({"server_id": server_id, "enabled": enabled, "customized": true}),
    );
    client
        .request(
            reqwest::Method::PUT,
            &format!("/api/agents/{agent_id}/mcp-config"),
            Some(json!({"servers": servers, "tools": tools})),
        )
        .await
}

async fn update_agent_mcp_tool(
    client: &ApiClient,
    agent_id: Uuid,
    server_id: Uuid,
    tool_name: &str,
    enabled: bool,
) -> AppResult<Value> {
    let servers = read_agent_override_items(client, agent_id, "mcp-config", "servers").await?;
    let mut tools = read_agent_override_items(client, agent_id, "mcp-config", "tools").await?;
    let server_key = server_id.to_string();
    replace_or_append_override(
        &mut tools,
        |item| {
            item.get("server_id").and_then(Value::as_str) == Some(server_key.as_str())
                && item.get("tool_name").and_then(Value::as_str) == Some(tool_name)
        },
        json!({"server_id": server_id, "tool_name": tool_name, "enabled": enabled}),
    );
    client
        .request(
            reqwest::Method::PUT,
            &format!("/api/agents/{agent_id}/mcp-config"),
            Some(json!({"servers": servers, "tools": tools})),
        )
        .await
}

async fn run_agent_skill(
    cli: &Cli,
    client: ApiClient,
    command: AgentSkillCommand,
) -> AppResult<()> {
    match command {
        AgentSkillCommand::Show { agent_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/agents/{agent_id}/skills-config"),
                    None,
                )
                .await?,
        ),
        AgentSkillCommand::Edit { agent_id, file } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::PUT,
                    &format!("/api/agents/{agent_id}/skills-config"),
                    Some(file_json(&file).await?),
                )
                .await?,
        ),
        AgentSkillCommand::Enable {
            agent_id,
            skill_name,
        } => print_value(
            cli,
            &update_agent_skill(&client, agent_id, &skill_name, true).await?,
        ),
        AgentSkillCommand::Disable {
            agent_id,
            skill_name,
        } => print_value(
            cli,
            &update_agent_skill(&client, agent_id, &skill_name, false).await?,
        ),
        AgentSkillCommand::GroupEnable { agent_id, group_id } => print_value(
            cli,
            &update_agent_skill_group(&client, agent_id, &group_id, true).await?,
        ),
        AgentSkillCommand::GroupDisable { agent_id, group_id } => print_value(
            cli,
            &update_agent_skill_group(&client, agent_id, &group_id, false).await?,
        ),
    }
}

async fn update_agent_skill(
    client: &ApiClient,
    agent_id: Uuid,
    skill_name: &str,
    enabled: bool,
) -> AppResult<Value> {
    let mut skills = read_agent_override_items(client, agent_id, "skills-config", "skills").await?;
    let groups = read_agent_override_items(client, agent_id, "skills-config", "groups").await?;
    let normalized = skill_name.trim().to_ascii_lowercase();
    replace_or_append_override(
        &mut skills,
        |item| item.get("name").and_then(Value::as_str) == Some(normalized.as_str()),
        json!({"name": normalized, "enabled": enabled}),
    );
    client
        .request(
            reqwest::Method::PUT,
            &format!("/api/agents/{agent_id}/skills-config"),
            Some(json!({"skills": skills, "groups": groups})),
        )
        .await
}

async fn update_agent_skill_group(
    client: &ApiClient,
    agent_id: Uuid,
    group_id: &str,
    enabled: bool,
) -> AppResult<Value> {
    let skills = read_agent_override_items(client, agent_id, "skills-config", "skills").await?;
    let mut groups = read_agent_override_items(client, agent_id, "skills-config", "groups").await?;
    replace_or_append_override(
        &mut groups,
        |item| {
            item.get("group_id").and_then(Value::as_str) == Some(group_id)
                || item.get("group").and_then(Value::as_str) == Some(group_id)
        },
        json!({"group": group_id, "group_id": group_id, "enabled": enabled}),
    );
    client
        .request(
            reqwest::Method::PUT,
            &format!("/api/agents/{agent_id}/skills-config"),
            Some(json!({"skills": skills, "groups": groups})),
        )
        .await
}

async fn run_agent_task(cli: &Cli, client: ApiClient, args: AgentRunArgs) -> AppResult<()> {
    let prompt = read_prompt(args.prompt).await?;
    let conversation_id = if let Some(id) = args.conversation {
        id
    } else {
        let conversation: Value = client
            .request(
                reqwest::Method::POST,
                &format!("/api/agents/{}/conversations", args.agent_id),
                Some(json!({"title": null})),
            )
            .await?;
        conversation["id"]
            .as_str()
            .ok_or_else(|| {
                AppError::Process("conversation response did not include id".to_string())
            })?
            .parse()
            .map_err(|_| AppError::Process("invalid conversation id".to_string()))?
    };
    let body = json!({"content": prompt, "stream": args.stream, "thinking_level": args.thinking_level.map(ThinkingLevel::from).unwrap_or_default()});
    let run: AgentRunInfo = client
        .request(
            reqwest::Method::POST,
            &format!("/api/conversations/{conversation_id}/runs"),
            Some(body),
        )
        .await?;
    if !run.status.is_empty() && run.status != "running" {
        return Err(AppError::Process(
            run.error
                .unwrap_or_else(|| "agent run did not start".to_string()),
        ));
    }
    client
        .stream_run(run.id, args.stream, cli.format, cli.quiet)
        .await
}

async fn run_provider(cli: &Cli, command: ProviderCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        ProviderCommand::List => print_value(
            cli,
            &summarize_providers(
                client
                    .request::<Value>(reqwest::Method::GET, "/api/model-providers", None)
                    .await?,
            ),
        ),
        ProviderCommand::Show { provider_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/model-providers/{provider_id}"),
                    None,
                )
                .await?,
        ),
        ProviderCommand::Create(args) => {
            let mut body = if let Some(path) = args.file {
                file_json(&path).await?
            } else {
                json!({})
            };
            set_if_present(&mut body, "name", args.name.map(Value::String));
            set_if_present(&mut body, "base_url", args.base_url.map(Value::String));
            set_if_present(
                &mut body,
                "models",
                (!args.models.is_empty()).then_some(json!(args.models)),
            );
            set_if_present(
                &mut body,
                "provider_type",
                args.provider_type.map(Value::String),
            );
            set_if_present(
                &mut body,
                "api_keys",
                provider_keys(args.api_keys, args.api_key_env).await?,
            );
            print_value(
                cli,
                &client
                    .request::<Value>(reqwest::Method::POST, "/api/model-providers", Some(body))
                    .await?,
            )
        }
        ProviderCommand::Edit(args) => {
            let mut body = if let Some(path) = args.file {
                file_json(&path).await?
            } else {
                json!({})
            };
            set_if_present(&mut body, "name", args.name.map(Value::String));
            set_if_present(&mut body, "base_url", args.base_url.map(Value::String));
            set_if_present(&mut body, "models", args.models.map(|v| json!(v)));
            set_if_present(
                &mut body,
                "provider_type",
                args.provider_type.map(Value::String),
            );
            set_if_present(
                &mut body,
                "api_keys",
                match (args.api_keys, args.api_key_env) {
                    (Some(v), env) => Some(json!(provider_keys(v, env).await?)),
                    (None, env) => provider_keys(Vec::new(), env).await?,
                },
            );
            print_value(
                cli,
                &client
                    .request::<Value>(
                        reqwest::Method::PUT,
                        &format!("/api/model-providers/{}", args.provider_id),
                        Some(body),
                    )
                    .await?,
            )
        }
        ProviderCommand::Delete { provider_id } => print_value(
            cli,
            &client
                .request_empty(
                    reqwest::Method::DELETE,
                    &format!("/api/model-providers/{provider_id}"),
                    None,
                )
                .await?,
        ),
        ProviderCommand::Test { provider_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/model-providers/{provider_id}/test"),
                    None,
                )
                .await?,
        ),
    }
}

async fn provider_keys(
    mut values: Vec<String>,
    env_name: Option<String>,
) -> AppResult<Option<Value>> {
    if let Some(name) = env_name {
        values.push(std::env::var(name).map_err(|_| {
            AppError::BadRequest("API key environment variable is missing".to_string())
        })?);
    }
    Ok((!values.is_empty()).then_some(json!(values)))
}

async fn run_model(cli: &Cli, command: ModelCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        ModelCommand::List { provider_id } => print_value(
            cli,
            &summarize_models(
                client
                    .request::<Value>(
                        reqwest::Method::GET,
                        &format!("/api/model-providers/{provider_id}/models"),
                        None,
                    )
                    .await?,
            ),
        ),
        ModelCommand::Test {
            provider_id,
            model_id,
            prompt,
        } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!(
                        "/api/model-providers/{provider_id}/models/{}/test",
                        urlencoding::encode(&model_id)
                    ),
                    Some(json!({"prompt": prompt.unwrap_or_else(|| "Reply with OK.".to_string())})),
                )
                .await?,
        ),
    }
}

async fn run_skill(cli: &Cli, command: SkillCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        SkillCommand::List => print_value(
            cli,
            &summarize_skills(
                client
                    .request::<Value>(reqwest::Method::GET, "/api/skills", None)
                    .await?,
            ),
        ),
        SkillCommand::Show { skill_name } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/skills/{}", urlencoding::encode(&skill_name)),
                    None,
                )
                .await?,
        ),
        SkillCommand::Content { skill_name, path } => {
            let path = path
                .map(|path| format!("?path={}", urlencoding::encode(&path)))
                .unwrap_or_default();
            print_value(
                cli,
                &client
                    .request::<Value>(
                        reqwest::Method::GET,
                        &format!(
                            "/api/skills/{}/content{path}",
                            urlencoding::encode(&skill_name)
                        ),
                        None,
                    )
                    .await?,
            )
        }
        SkillCommand::Run {
            skill_name,
            task,
            agent,
        } => {
            let agent_id = agent
                .or(find_builtin_agent(&client).await?)
                .ok_or_else(|| {
                    AppError::BadRequest("no Agent is available for Skill execution".to_string())
                })?;
            run_agent_task(
                cli,
                client,
                AgentRunArgs {
                    agent_id,
                    prompt: Some(format!("/{skill_name} {}", read_prompt(task).await?)),
                    conversation: None,
                    stream: true,
                    thinking_level: None,
                },
            )
            .await
        }
        SkillCommand::Reload => print_value(
            cli,
            &client
                .request::<Value>(reqwest::Method::POST, "/api/skills/reload", None)
                .await?,
        ),
        SkillCommand::Remove { skill_name } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::DELETE,
                    &format!("/api/skills/{}", urlencoding::encode(&skill_name)),
                    None,
                )
                .await?,
        ),
        SkillCommand::Install {
            source,
            proxy_id,
            group_id,
        } => {
            let task: Value = client
                .request(
                    reqwest::Method::POST,
                    "/api/skills/import",
                    Some(json!({"source": source, "proxy_id": proxy_id, "group_id": group_id})),
                )
                .await?;
            let task_id = task["id"].as_str().ok_or_else(|| {
                AppError::Process("Skill import response did not include id".to_string())
            })?;
            let poll = async {
                loop {
                    let tasks: Vec<Value> = client
                        .request(reqwest::Method::GET, "/api/skills/import-tasks", None)
                        .await?;
                    if let Some(current) = tasks.into_iter().find(|item| item["id"] == task_id) {
                        let status = current["status"].as_str().unwrap_or_default();
                        if matches!(status, "completed" | "failed" | "cancelled") {
                            return print_value(cli, &current);
                        }
                    }
                    sleep(Duration::from_millis(400)).await;
                }
                #[allow(unreachable_code)]
                Ok::<(), AppError>(())
            };
            timeout(IMPORT_POLL_TIMEOUT, poll).await.map_err(|_| {
                AppError::Process("timed out waiting for Skill import".to_string())
            })??;
            Ok(())
        }
    }
}

async fn find_builtin_agent(client: &ApiClient) -> AppResult<Option<Uuid>> {
    let agents: Vec<Value> = client
        .request(reqwest::Method::GET, "/api/agents", None)
        .await?;
    Ok(agents
        .into_iter()
        .find(|agent| agent["is_builtin"] == true)
        .and_then(|agent| agent["id"].as_str()?.parse().ok()))
}

async fn run_mcp(cli: &Cli, command: McpCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        McpCommand::List | McpCommand::Show { server_id: None } => print_value(
            cli,
            &summarize_mcp_servers(
                client
                    .request::<Value>(reqwest::Method::GET, "/api/tools/mcp", None)
                    .await?,
            ),
        ),
        McpCommand::Show {
            server_id: Some(server_id),
        } => print_value(
            cli,
            &summarize_mcp_server(
                client
                    .request::<Value>(
                        reqwest::Method::GET,
                        &format!("/api/tools/mcp/{server_id}"),
                        None,
                    )
                    .await?,
            ),
        ),
        McpCommand::Tools { server_id } => {
            let server = client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/tools/mcp/{server_id}"),
                    None,
                )
                .await?;
            print_value(cli, &summarize_mcp_tools(&server))
        }
        McpCommand::Tool {
            command:
                McpToolCommand::Show {
                    server_id,
                    tool_name,
                },
        } => {
            let server = client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/tools/mcp/{server_id}"),
                    None,
                )
                .await?;
            let tool = find_mcp_tool(&server, &tool_name).ok_or(AppError::NotFound)?;
            print_value(cli, &tool)
        }
        McpCommand::Add(args) => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    "/api/tools/mcp",
                    Some(file_json(&args.file).await?),
                )
                .await?,
        ),
        McpCommand::Edit { server_id, file } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::PUT,
                    &format!("/api/tools/mcp/{server_id}"),
                    Some(file_json(&file).await?),
                )
                .await?,
        ),
        McpCommand::Remove { server_id } => print_value(
            cli,
            &client
                .request_empty(
                    reqwest::Method::DELETE,
                    &format!("/api/tools/mcp/{server_id}"),
                    None,
                )
                .await?,
        ),
        McpCommand::Enable { server_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::PUT,
                    &format!("/api/tools/mcp/{server_id}/enabled"),
                    Some(json!({"enabled": true})),
                )
                .await?,
        ),
        McpCommand::Disable { server_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::PUT,
                    &format!("/api/tools/mcp/{server_id}/enabled"),
                    Some(json!({"enabled": false})),
                )
                .await?,
        ),
        McpCommand::Test { server_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/tools/mcp/{server_id}/test"),
                    None,
                )
                .await?,
        ),
        McpCommand::Reload {
            server_id: Some(server_id),
        } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/tools/mcp/{server_id}/refresh"),
                    None,
                )
                .await?,
        ),
        McpCommand::Reload { server_id: None } => {
            let servers: Vec<Value> = client
                .request(reqwest::Method::GET, "/api/tools/mcp", None)
                .await?;
            let mut results = Vec::new();
            for server in servers {
                if let Some(id) = server["id"].as_str() {
                    results.push(
                        client
                            .request::<Value>(
                                reqwest::Method::POST,
                                &format!("/api/tools/mcp/{id}/refresh"),
                                None,
                            )
                            .await?,
                    );
                }
            }
            print_value(cli, &results)
        }
    }
}

async fn run_proxy(cli: &Cli, command: ProxyCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        ProxyCommand::List => print_value(
            cli,
            &summarize_proxies(
                client
                    .request::<Value>(reqwest::Method::GET, "/api/ip-proxies", None)
                    .await?,
            ),
        ),
        ProxyCommand::Show { proxy_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/ip-proxies/{proxy_id}"),
                    None,
                )
                .await?,
        ),
        ProxyCommand::Add(args) => {
            let mut body = if let Some(path) = args.file {
                file_json(&path).await?
            } else {
                json!({})
            };
            set_if_present(&mut body, "name", args.name.map(Value::String));
            set_if_present(&mut body, "url", args.url.map(Value::String));
            print_value(
                cli,
                &client
                    .request::<Value>(reqwest::Method::POST, "/api/ip-proxies", Some(body))
                    .await?,
            )
        }
        ProxyCommand::Edit(args) => {
            let mut body = if let Some(path) = args.file {
                file_json(&path).await?
            } else {
                json!({})
            };
            set_if_present(&mut body, "name", args.name.map(Value::String));
            set_if_present(&mut body, "url", args.url.map(Value::String));
            set_if_present(&mut body, "enabled", args.enabled.map(Value::Bool));
            print_value(
                cli,
                &client
                    .request::<Value>(
                        reqwest::Method::PUT,
                        &format!("/api/ip-proxies/{}", args.proxy_id),
                        Some(body),
                    )
                    .await?,
            )
        }
        ProxyCommand::Remove { proxy_id } => print_value(
            cli,
            &client
                .request_empty(
                    reqwest::Method::DELETE,
                    &format!("/api/ip-proxies/{proxy_id}"),
                    None,
                )
                .await?,
        ),
        ProxyCommand::Test { proxy_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/ip-proxies/{proxy_id}/test"),
                    None,
                )
                .await?,
        ),
    }
}

async fn read_prompt_input(prompt: Option<String>, file: Option<PathBuf>) -> AppResult<String> {
    if let Some(path) = file {
        let value = tokio::fs::read_to_string(path).await?.trim().to_string();
        if value.is_empty() {
            return Err(AppError::BadRequest("prompt file is empty".to_string()));
        }
        return Ok(value);
    }
    read_prompt(prompt).await
}

fn parse_interval_seconds(value: &str) -> AppResult<u64> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::BadRequest("interval cannot be empty".to_string()));
    }
    let (number, multiplier) = match value.chars().last() {
        Some('s') => (&value[..value.len() - 1], 1),
        Some('m') => (&value[..value.len() - 1], 60),
        Some('h') => (&value[..value.len() - 1], 60 * 60),
        Some('d') => (&value[..value.len() - 1], 24 * 60 * 60),
        _ => (value, 1),
    };
    let number = number.parse::<u64>().map_err(|_| {
        AppError::BadRequest("interval must look like 30s, 10m, 2h, or 1d".to_string())
    })?;
    number
        .checked_mul(multiplier)
        .ok_or_else(|| AppError::BadRequest("interval is too large".to_string()))
}

async fn run_cron(cli: &Cli, command: CronCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        CronCommand::List => print_value(
            cli,
            &client
                .request::<Value>(reqwest::Method::GET, "/api/cron", None)
                .await?,
        ),
        CronCommand::Show { task_id } => print_value(
            cli,
            &client
                .request::<Value>(reqwest::Method::GET, &format!("/api/cron/{task_id}"), None)
                .await?,
        ),
        CronCommand::Create(args) => {
            let prompt = read_prompt_input(args.prompt, args.prompt_file).await?;
            print_value(
                cli,
                &client
                    .request::<Value>(
                        reqwest::Method::POST,
                        "/api/cron",
                        Some(json!({
                            "agent_id": args.agent,
                            "source_conversation_id": args.conversation,
                            "schedule": args.schedule,
                            "prompt": prompt,
                            "timezone_offset": args.timezone,
                        })),
                    )
                    .await?,
            )
        }
        CronCommand::Edit(args) => {
            let prompt = match (args.prompt, args.prompt_file) {
                (None, None) => None,
                (prompt, file) => Some(read_prompt_input(prompt, file).await?),
            };
            print_value(
                cli,
                &client
                    .request::<Value>(
                        reqwest::Method::PATCH,
                        &format!("/api/cron/{}", args.task_id),
                        Some(json!({
                            "schedule": args.schedule,
                            "prompt": prompt,
                            "timezone_offset": args.timezone,
                        })),
                    )
                    .await?,
            )
        }
        CronCommand::Pause { task_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/cron/{task_id}/pause"),
                    None,
                )
                .await?,
        ),
        CronCommand::Resume { task_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/cron/{task_id}/resume"),
                    None,
                )
                .await?,
        ),
        CronCommand::Run { task_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/cron/{task_id}/run"),
                    None,
                )
                .await?,
        ),
        CronCommand::Remove { task_id } => print_value(
            cli,
            &client
                .request_empty(
                    reqwest::Method::DELETE,
                    &format!("/api/cron/{task_id}"),
                    None,
                )
                .await?,
        ),
        CronCommand::Executions { task_id } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/cron/{task_id}/executions"),
                    None,
                )
                .await?,
        ),
    }
}

async fn run_heartbeat(cli: &Cli, command: HeartbeatCommand) -> AppResult<()> {
    let (client, _child) = ApiClient::connect(cli).await?;
    match command {
        HeartbeatCommand::Show { conversation } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::GET,
                    &format!("/api/conversations/{conversation}/heartbeat"),
                    None,
                )
                .await?,
        ),
        HeartbeatCommand::Set(args) => {
            let prompt = read_prompt_input(args.prompt, args.prompt_file).await?;
            let interval_seconds = parse_interval_seconds(&args.interval)?;
            print_value(
                cli,
                &client
                    .request::<Value>(
                        reqwest::Method::PUT,
                        &format!("/api/conversations/{}/heartbeat", args.conversation),
                        Some(json!({
                            "interval_seconds": interval_seconds,
                            "prompt": prompt,
                        })),
                    )
                    .await?,
            )
        }
        HeartbeatCommand::Pause { conversation } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/conversations/{conversation}/heartbeat/pause"),
                    None,
                )
                .await?,
        ),
        HeartbeatCommand::Resume { conversation } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/conversations/{conversation}/heartbeat/resume"),
                    None,
                )
                .await?,
        ),
        HeartbeatCommand::Run { conversation } => print_value(
            cli,
            &client
                .request::<Value>(
                    reqwest::Method::POST,
                    &format!("/api/conversations/{conversation}/heartbeat/run"),
                    None,
                )
                .await?,
        ),
        HeartbeatCommand::Remove { conversation } => print_value(
            cli,
            &client
                .request_empty(
                    reqwest::Method::DELETE,
                    &format!("/api/conversations/{conversation}/heartbeat"),
                    None,
                )
                .await?,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_cli_is_the_production_server() {
        let cli = Cli::try_parse_from(["weiagent"]).unwrap();
        assert!(cli.is_server_command());
        assert!(!cli.is_dev());
    }

    #[test]
    fn dev_is_a_server_command() {
        let cli = Cli::try_parse_from(["weiagent", "dev"]).unwrap();
        assert!(cli.is_server_command());
        assert!(cli.is_dev());
    }

    #[test]
    fn agent_full_show_parses() {
        let agent_id = Uuid::nil().to_string();
        let cli = Cli::try_parse_from(["weiagent", "agent", "show", &agent_id, "--full"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Agent {
                command: AgentCommand::Show { full: true, .. }
            })
        ));
    }

    #[test]
    fn agent_create_full_output_flag_parses() {
        let cli = Cli::try_parse_from([
            "weiagent",
            "agent",
            "create",
            "--file",
            "AGENT.json",
            "--full",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Agent {
                command: AgentCommand::Create(AgentCreateArgs { full: true, .. })
            })
        ));
    }

    #[test]
    fn agent_edit_full_output_flag_parses() {
        let agent_id = Uuid::nil().to_string();
        let cli = Cli::try_parse_from([
            "weiagent", "agent", "edit", &agent_id, "--model", "MODEL_ID", "--full",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Agent {
                command: AgentCommand::Edit(AgentEditArgs { full: true, .. })
            })
        ));
    }

    #[test]
    fn agent_profile_and_tools_edits_parse() {
        let agent_id = Uuid::nil().to_string();
        let profile = Cli::try_parse_from([
            "weiagent",
            "agent",
            "profile",
            "edit",
            &agent_id,
            "--file",
            "PROFILE.json",
        ])
        .unwrap();
        assert!(matches!(
            profile.command,
            Some(Command::Agent {
                command: AgentCommand::Profile {
                    command: AgentProfileCommand::Edit { .. }
                }
            })
        ));

        let tools = Cli::try_parse_from([
            "weiagent",
            "agent",
            "tools",
            "edit",
            &agent_id,
            "--file",
            "TOOLS.json",
        ])
        .unwrap();
        assert!(matches!(
            tools.command,
            Some(Command::Agent {
                command: AgentCommand::Tools {
                    command: AgentToolsCommand::Edit { .. }
                }
            })
        ));
    }

    #[test]
    fn agent_mcp_and_skill_shortcuts_parse() {
        let agent_id = Uuid::nil().to_string();
        let server_id = Uuid::nil().to_string();
        let mcp = Cli::try_parse_from([
            "weiagent",
            "agent",
            "mcp",
            "tool-enable",
            &agent_id,
            &server_id,
            "search",
        ])
        .unwrap();
        assert!(matches!(
            mcp.command,
            Some(Command::Agent {
                command: AgentCommand::Mcp {
                    command: AgentMcpCommand::ToolEnable { .. }
                }
            })
        ));

        let skill = Cli::try_parse_from([
            "weiagent",
            "agent",
            "skill",
            "group-disable",
            &agent_id,
            "builtin",
        ])
        .unwrap();
        assert!(matches!(
            skill.command,
            Some(Command::Agent {
                command: AgentCommand::Skill {
                    command: AgentSkillCommand::GroupDisable { .. }
                }
            })
        ));
    }

    #[test]
    fn complete_agent_document_options_parse() {
        let agent_id = Uuid::nil().to_string();
        let cli = Cli::try_parse_from([
            "weiagent",
            "agent",
            "edit",
            &agent_id,
            "--file",
            "AGENT.json",
            "--profile-file",
            "PROFILE.json",
            "--tools-file",
            "TOOLS.json",
            "--mcp-file",
            "MCP.json",
            "--skills-file",
            "SKILLS.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Agent {
                command: AgentCommand::Edit(AgentEditArgs {
                    file: Some(_),
                    profile_file: Some(_),
                    tools_file: Some(_),
                    mcp_file: Some(_),
                    skills_file: Some(_),
                    ..
                })
            })
        ));
    }

    #[test]
    fn development_data_directory_is_discovered_from_workspace_ancestors() {
        let project = PathBuf::from("repo");
        let workspace = project.join("data").join("workspace");
        let mut candidates = Vec::new();

        push_ancestor_data_dirs(&mut candidates, &workspace);

        assert!(candidates.contains(&project.join("data")));
    }

    #[test]
    fn duplicate_data_directory_candidates_are_ignored() {
        let mut candidates = Vec::new();
        let data_dir = PathBuf::from("data");

        push_unique_path(&mut candidates, data_dir.clone());
        push_unique_path(&mut candidates, data_dir);

        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn skill_content_command_supports_progressive_path_disclosure() {
        let cli = Cli::try_parse_from([
            "weiagent",
            "skill",
            "content",
            "weiagentcli",
            "--path",
            "examples/advanced.md",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Skill {
                command: SkillCommand::Content { path: Some(_), .. }
            })
        ));
    }

    #[test]
    fn list_projections_drop_detail_payloads() {
        let mcp = summarize_mcp_servers(json!([
            {
                "id": "SERVER_ID",
                "name": "Search",
                "enabled": true,
                "status": "ready",
                "provider": "demo",
                "tools": [{"name": "search", "input_schema": {"type": "object"}}]
            }
        ]));
        assert_eq!(mcp[0]["name"], "Search");
        assert_eq!(mcp[0]["tool_count"], 1);
        assert!(mcp[0].get("tools").is_none());

        let skills = summarize_skills(json!({
            "defaults": [{
                "name": "weiagentcli",
                "version": "1.0.0",
                "group": "built-in",
                "enabled": true,
                "is_builtin": true,
                "scripts": [{"path": "run.ps1"}],
                "references": [{"path": "large.md"}]
            }],
            "groups": []
        }));
        assert_eq!(skills["skills"][0]["name"], "weiagentcli");
        assert!(skills["skills"][0].get("scripts").is_none());
        assert!(skills["skills"][0].get("references").is_none());
    }

    #[test]
    fn mcp_projections_follow_progressive_disclosure() {
        let server = json!({
            "id": "SERVER_ID",
            "name": "Search",
            "transport": {"type": "stdio", "command": "MCP_COMMAND"},
            "diagnostics": {"protocol_version": "2025-06-18"},
            "tools": [{
                "tool_id": "TOOL_ID",
                "name": "search",
                "qualified_name": "Search__search",
                "title": "Search",
                "description": "Searches the index",
                "enabled": true,
                "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}}
            }]
        });

        let server_summary = summarize_mcp_server(server.clone());
        assert!(server_summary.get("tools").is_none());

        let tools = summarize_mcp_tools(&server);
        assert_eq!(tools[0]["name"], "search");
        assert!(tools[0].get("description").is_none());
        assert!(tools[0].get("input_schema").is_none());

        let tool = find_mcp_tool(&server, "Search__search").unwrap();
        assert_eq!(
            tool["input_schema"]["properties"]["query"]["type"],
            "string"
        );
    }

    #[test]
    fn detail_commands_parse_for_each_indexed_resource() {
        let id = Uuid::nil().to_string();
        for args in [
            vec!["weiagent", "provider", "show", &id],
            vec!["weiagent", "mcp", "show"],
            vec!["weiagent", "mcp", "show", &id],
            vec!["weiagent", "mcp", "tools", &id],
            vec!["weiagent", "mcp", "tool", "show", &id, "TOOL_NAME"],
            vec!["weiagent", "proxy", "show", &id],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
    }

    #[test]
    fn scheduling_commands_parse() {
        let agent_id = Uuid::nil().to_string();
        let cron = Cli::try_parse_from([
            "weiagent",
            "cron",
            "create",
            "--agent",
            &agent_id,
            "--schedule",
            "0 9 * * *",
            "check",
        ])
        .unwrap();
        assert!(matches!(
            cron.command,
            Some(Command::Cron {
                command: CronCommand::Create(_)
            })
        ));
        let conversation_id = Uuid::nil().to_string();
        let heartbeat = Cli::try_parse_from([
            "weiagent",
            "heartbeat",
            "set",
            "--conversation",
            &conversation_id,
            "--interval",
            "10m",
            "check",
        ])
        .unwrap();
        assert!(matches!(
            heartbeat.command,
            Some(Command::Heartbeat {
                command: HeartbeatCommand::Set(_)
            })
        ));
    }
}
