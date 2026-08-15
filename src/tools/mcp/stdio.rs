// Internal MCP transport/auth helpers split out of the manager module.
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Instant,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
    task::JoinHandle,
    time::{Duration, timeout},
};
use uuid::Uuid;

use super::auth::McpAuthRuntime;
use super::env::expand_mcp_value;
use super::protocol::{
    MAX_STDIO_FRAME_BYTES, McpHandshake, McpRpcResponse, STDIO_MAX_RESPONSE_WAIT_ITERATIONS,
    format_mcp_stderr, mcp_initialize_request, mcp_timeout_error, mcp_timeout_ms,
    parse_mcp_handshake, should_try_stdio_protocol_fallback,
};
use crate::{
    domain::{McpServer, McpTransport},
    error::{AppError, AppResult},
    tools::{runtime::EnvironmentRuntimeManager, terminal::managed_path_from},
};

pub(super) struct StdioSessionEntry {
    pub(super) signature: String,
    pub(super) session: Arc<Mutex<StdioSession>>,
    pub(super) last_used: Mutex<Instant>,
}

impl StdioSessionEntry {
    pub(super) fn new(signature: String, session: Arc<Mutex<StdioSession>>) -> Self {
        Self {
            signature,
            session,
            last_used: Mutex::new(Instant::now()),
        }
    }

    pub(super) async fn touch(&self) {
        *self.last_used.lock().await = Instant::now();
    }
}

pub(super) fn stdio_server_signature(server: &McpServer) -> String {
    serde_json::to_string(&server.transport)
        .unwrap_or_else(|_| format!("{}:{}", server.id, server.name))
}

pub(super) async fn rpc_stdio_once(
    auth: &Arc<McpAuthRuntime>,
    environment: &Arc<EnvironmentRuntimeManager>,
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<McpRpcResponse> {
    let timeout_duration =
        Duration::from_millis(mcp_timeout_ms(&server.transport).clamp(1_000, 120_000));
    let mut session =
        StdioSession::start(server, timeout_duration, auth.clone(), environment.clone()).await?;
    let result = if let Some((method, params)) = request {
        session.request(method, params, timeout_duration).await
    } else {
        Ok(json!({}))
    };
    let handshake = session.handshake.clone();
    session.stop().await;
    result.map(|value| McpRpcResponse {
        value,
        handshake: Some(handshake),
    })
}

pub(super) struct StdioSession {
    pub(super) child: tokio::process::Child,
    pub(super) stdin: tokio::process::ChildStdin,
    pub(super) reader: BufReader<tokio::process::ChildStdout>,
    pub(super) protocol: StdioProtocol,
    pub(super) next_id: i64,
    pub(super) stderr_task: Option<JoinHandle<()>>,
    pub(super) stderr_output: Arc<Mutex<Vec<String>>>,
    pub(super) auth: Arc<McpAuthRuntime>,
    pub(super) server_id: Uuid,
    pub(super) handshake: McpHandshake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StdioProtocol {
    ContentLength,
    JsonLines,
}

impl StdioSession {
    pub(super) async fn start(
        server: &McpServer,
        timeout_duration: Duration,
        auth: Arc<McpAuthRuntime>,
        environment: Arc<EnvironmentRuntimeManager>,
    ) -> AppResult<Self> {
        auth.mark_starting(server.id).await;
        let primary = Self::start_with_protocol(
            server,
            timeout_duration,
            auth.clone(),
            environment.clone(),
            StdioProtocol::JsonLines,
        )
        .await;
        match primary {
            Ok(session) => {
                auth.mark_connected(server.id).await;
                Ok(session)
            }
            Err(primary_error) if should_try_stdio_protocol_fallback(&primary_error) => {
                let fallback = Self::start_with_protocol(
                    server,
                    timeout_duration,
                    auth.clone(),
                    environment,
                    StdioProtocol::ContentLength,
                )
                .await;
                match fallback {
                    Ok(session) => {
                        auth.mark_connected(server.id).await;
                        Ok(session)
                    }
                    Err(fallback_error) => {
                        let error = AppError::Process(format!(
                            "stdio protocol negotiation failed (content-length: {primary_error}; json-lines: {fallback_error})"
                        ));
                        auth.mark_error(server.id, error.to_string()).await;
                        Err(error)
                    }
                }
            }
            Err(error) => {
                auth.mark_error(server.id, error.to_string()).await;
                Err(error)
            }
        }
    }

    pub(super) async fn start_with_protocol(
        server: &McpServer,
        timeout_duration: Duration,
        auth: Arc<McpAuthRuntime>,
        environment: Arc<EnvironmentRuntimeManager>,
        protocol: StdioProtocol,
    ) -> AppResult<Self> {
        let mut session = Self::spawn(server, auth.clone(), environment, protocol).await?;
        match timeout(timeout_duration, session.initialize()).await {
            Ok(Ok(handshake)) => {
                session.handshake = handshake;
                Ok(session)
            }
            Ok(Err(error)) => {
                session.stop().await;
                Err(error)
            }
            Err(_) => {
                session.stop().await;
                Err(mcp_timeout_error(timeout_duration))
            }
        }
    }

    pub(super) async fn spawn(
        server: &McpServer,
        auth: Arc<McpAuthRuntime>,
        environment: Arc<EnvironmentRuntimeManager>,
        protocol: StdioProtocol,
    ) -> AppResult<Self> {
        let McpTransport::Stdio {
            command,
            args,
            cwd,
            env: _,
            ..
        } = &server.transport
        else {
            unreachable!()
        };
        if command.trim().is_empty() {
            return Err(AppError::BadRequest(
                "MCP stdio command is required".to_string(),
            ));
        }
        let expanded_command = expand_mcp_value(command);
        let expanded_args = args
            .iter()
            .map(|arg| expand_mcp_value(arg))
            .collect::<Vec<_>>();
        let (program, launch_args) = normalize_stdio_launch(&expanded_command, &expanded_args);
        let managed_env = auth.managed_env(server);
        let configured_path = managed_env.get("PATH").map(OsStr::new);
        let inherited_path = std::env::var_os("PATH");
        let base_path = configured_path.or(inherited_path.as_deref());
        let managed_path = managed_path_from(environment.as_ref(), base_path).await;
        let command_path = managed_path
            .as_deref()
            .or(configured_path)
            .or(inherited_path.as_deref());
        let mut process = build_stdio_command(&program, &launch_args, command_path);
        process.env_clear();
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd
            .as_deref()
            .map(expand_mcp_value)
            .filter(|value| !value.trim().is_empty())
        {
            process.current_dir(cwd);
        }
        process.envs(&managed_env);
        if let Some(path) = managed_path.as_ref() {
            process.env("PATH", path);
        }
        configure_mcp_spawn_command(&mut process);
        let mut child = process
            .spawn()
            .map_err(|error| {
                AppError::Process(format!(
                    "MCP process failed to start: {error}; command={program:?}; args={launch_args:?}; {}",
                    stdio_launch_debug(&program, command_path)
                ))
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Process("MCP stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Process("MCP stdout unavailable".to_string()))?;
        let stderr_output = Arc::new(Mutex::new(Vec::new()));
        let stderr_task = child.stderr.take().map(|stderr| {
            let auth = auth.clone();
            let server_id = server.id;
            let stderr_output = stderr_output.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = line.trim();
                    if !line.is_empty() {
                        let mut output = stderr_output.lock().await;
                        output.push(line.to_string());
                        if output.len() > 20 {
                            output.remove(0);
                        }
                        drop(output);
                        auth.handle_stderr_line(server_id, line).await;
                    }
                }
            })
        });
        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            protocol,
            next_id: 2,
            stderr_task,
            stderr_output,
            auth,
            server_id: server.id,
            handshake: McpHandshake::default(),
        })
    }

    pub(super) async fn read_response(&mut self, expected_id: i64) -> AppResult<Value> {
        match read_stdio_response(&mut self.reader, self.protocol, expected_id).await {
            Ok(value) => Ok(value),
            Err(AppError::Process(message)) if message == "MCP process closed before response" => {
                self.drain_stderr_after_exit().await;
                let exit_detail = self.child.try_wait().ok().flatten().map(|status| {
                    status
                        .code()
                        .map(|code| format!("exit code {code}"))
                        .unwrap_or_else(|| "terminated by signal".to_string())
                });
                let stderr_detail = format_mcp_stderr(&self.stderr_output.lock().await);
                let details = [
                    exit_detail,
                    stderr_detail.map(|output| format!("stderr:\n{output}")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                if details.is_empty() {
                    Err(AppError::Process(message))
                } else {
                    Err(AppError::Process(format!(
                        "{message} ({})",
                        details.join("; ")
                    )))
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn drain_stderr_after_exit(&mut self) {
        let exited = match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => timeout(Duration::from_millis(250), self.child.wait())
                .await
                .is_ok(),
            Err(_) => false,
        };
        if exited {
            if let Some(task) = self.stderr_task.take() {
                let _ = task.await;
            }
        }
    }

    pub(super) async fn initialize(&mut self) -> AppResult<McpHandshake> {
        send_stdio_message(&mut self.stdin, mcp_initialize_request(), self.protocol).await?;
        let result = self.read_response(1).await?;
        send_stdio_message(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            self.protocol,
        )
        .await?;
        Ok(parse_mcp_handshake(&result))
    }

    pub(super) async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_duration: Duration,
    ) -> AppResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let result = timeout(timeout_duration, async {
            send_stdio_message(
                &mut self.stdin,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                self.protocol,
            )
            .await?;
            self.read_response(id).await
        })
        .await;
        match result {
            Ok(Ok(value)) => {
                self.auth.mark_connected(self.server_id).await;
                Ok(value)
            }
            Ok(Err(error)) => {
                self.auth
                    .mark_error(self.server_id, error.to_string())
                    .await;
                Err(error)
            }
            Err(_) => {
                let error = mcp_timeout_error(timeout_duration);
                self.auth
                    .mark_timeout(self.server_id, error.to_string())
                    .await;
                Err(error)
            }
        }
    }

    pub(super) async fn stop(&mut self) {
        if let Some(pid) = self.child.id() {
            let _ = crate::tools::process_job::terminate_process_tree(pid).await;
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

impl Drop for StdioSession {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            let _ = self.child.start_kill();
            tracing::debug!(pid, "stopping dropped MCP stdio process");
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

pub(super) fn normalize_stdio_args(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|arg| {
            let trimmed = arg.trim();
            let Some(separator) = trimmed.find(char::is_whitespace) else {
                return vec![arg.clone()];
            };
            let option = &trimmed[..separator];
            let value = trimmed[separator..].trim();
            if !option.starts_with('-') || value.is_empty() {
                return vec![arg.clone()];
            }
            vec![option.to_string(), value.to_string()]
        })
        .collect()
}

pub(super) fn normalize_stdio_launch(command: &str, args: &[String]) -> (String, Vec<String>) {
    let mut command_parts = split_stdio_command(command);
    let program = command_parts
        .first()
        .cloned()
        .unwrap_or_else(|| command.trim().to_string());
    if !command_parts.is_empty() {
        command_parts.remove(0);
    }
    command_parts.extend(normalize_stdio_args(args));
    (program, command_parts)
}

pub(super) fn split_stdio_command(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for character in command.chars() {
        match character {
            '"' | '\'' if quote.is_none() => quote = Some(character),
            '"' | '\'' if quote == Some(character) => quote = None,
            character if character.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

pub(super) fn build_stdio_command(program: &str, args: &[String], path: Option<&OsStr>) -> Command {
    #[cfg(windows)]
    if (program.eq_ignore_ascii_case("npx") || program.eq_ignore_ascii_case("npm"))
        && let Some(command) = build_node_package_manager_command_for_program(program, args, path)
    {
        return command;
    }

    #[cfg(windows)]
    if let Some(script) = resolve_windows_script(program, path) {
        let mut command = Command::new(windows_command_interpreter());
        command
            .args(["/D", "/S", "/C"])
            .arg(build_windows_script(&script, args));
        return command;
    }

    let mut command = Command::new(program);
    command.args(args);
    command
}

#[cfg(windows)]
pub(super) fn build_node_package_manager_command(
    script: &Path,
    args: &[String],
    path: Option<&OsStr>,
) -> Option<Command> {
    let stem = script.file_stem()?.to_string_lossy();
    if !stem.eq_ignore_ascii_case("npx") && !stem.eq_ignore_ascii_case("npm") {
        return None;
    }
    let node = resolve_windows_executable("node", path)?;
    let cli = script
        .parent()?
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join(format!("{stem}-cli.js"));
    if !cli.is_file() {
        return None;
    }
    let mut command = Command::new(node);
    command.arg(cli).args(args);
    Some(command)
}

#[cfg(windows)]
pub(super) fn build_node_package_manager_command_for_program(
    program: &str,
    args: &[String],
    path: Option<&OsStr>,
) -> Option<Command> {
    let Some(script) = resolve_windows_script(program, path) else {
        tracing::warn!(program, "MCP package manager shim was not found");
        return None;
    };
    let command = build_node_package_manager_command(&script, args, path);
    if command.is_none() {
        tracing::warn!(program, script = %script.display(), "MCP package manager shim could not be converted to a direct Node command");
    }
    command
}

#[cfg(windows)]
pub(super) fn resolve_windows_executable(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let names = [
        PathBuf::from(program),
        PathBuf::from(format!("{program}.exe")),
    ];
    let mut directories = Vec::new();
    if let Some(path) = path {
        directories.extend(std::env::split_paths(path));
    }
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    directories.into_iter().find_map(|directory| {
        names.iter().find_map(|name| {
            let candidate = directory.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(windows)]
pub(super) fn windows_command_interpreter() -> PathBuf {
    let candidates = [
        std::env::var_os("ComSpec").map(PathBuf::from),
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("cmd.exe")),
        Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("cmd.exe"))
}

#[cfg(windows)]
pub(super) fn resolve_windows_script(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let raw = PathBuf::from(program);
    let has_path = raw.components().count() > 1 || raw.extension().is_some();
    let names = if has_path {
        vec![raw]
    } else {
        vec![
            PathBuf::from(program),
            PathBuf::from(format!("{program}.exe")),
            PathBuf::from(format!("{program}.cmd")),
            PathBuf::from(format!("{program}.bat")),
        ]
    };

    let is_script = |candidate: &Path| {
        candidate.is_file()
            && candidate.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            })
    };
    if let Some(candidate) = names.iter().find(|candidate| is_script(candidate)) {
        return Some(candidate.clone());
    }

    let inherited_path = std::env::var_os("PATH");
    let mut directories = Vec::new();
    if let Some(path) = path {
        directories.extend(std::env::split_paths(path));
    }
    if let Some(path) = inherited_path.as_deref() {
        directories.extend(std::env::split_paths(path));
    }
    for variable in [
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
    ] {
        if let Some(root) = std::env::var_os(variable).map(PathBuf::from) {
            directories.push(root.join("npm"));
            directories.push(root.join("nodejs"));
        }
    }
    if let Some(user_profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        directories.push(user_profile.join(r"AppData\Roaming\npm"));
        directories.push(user_profile.join(r"AppData\Local\npm"));
    }
    for directory in directories {
        for name in &names {
            let candidate = directory.join(name);
            if is_script(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
pub(super) fn stdio_launch_debug(program: &str, path: Option<&OsStr>) -> String {
    let resolved = resolve_windows_script(program, path)
        .map(|value| crate::fsutil::display_path(&value))
        .unwrap_or_else(|| "<not resolved>".to_string());
    format!(
        "resolved_script={resolved:?}; comspec={:?}",
        windows_command_interpreter()
    )
}

#[cfg(not(windows))]
pub(super) fn stdio_launch_debug(_program: &str, _path: Option<&OsStr>) -> String {
    "resolved_script=<not applicable>".to_string()
}

#[cfg(windows)]
pub(super) fn build_windows_script(program: &Path, args: &[String]) -> String {
    let mut command = format!("call {}", quote_windows_cmd_arg(&program.to_string_lossy()));
    for arg in args {
        command.push(' ');
        command.push_str(&quote_windows_cmd_arg(arg));
    }
    command
}

#[cfg(windows)]
pub(super) fn quote_windows_cmd_arg(value: &str) -> String {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

#[cfg(windows)]
pub(super) fn configure_mcp_spawn_command(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(super) fn configure_mcp_spawn_command(_command: &mut Command) {}

pub(super) async fn send_stdio_message(
    stdin: &mut tokio::process::ChildStdin,
    value: Value,
    protocol: StdioProtocol,
) -> AppResult<()> {
    let body = serde_json::to_vec(&value)?;
    match protocol {
        StdioProtocol::ContentLength => {
            let header = format!("Content-Length: {}\r\n\r\n", body.len());
            stdin.write_all(header.as_bytes()).await?;
            stdin.write_all(&body).await?;
        }
        StdioProtocol::JsonLines => {
            stdin.write_all(&body).await?;
            stdin.write_all(b"\n").await?;
        }
    }
    stdin.flush().await?;
    Ok(())
}

pub(super) async fn read_stdio_response(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    protocol: StdioProtocol,
    expected_id: i64,
) -> AppResult<Value> {
    for _ in 0..STDIO_MAX_RESPONSE_WAIT_ITERATIONS {
        let value = read_stdio_message(reader, protocol).await?;
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(AppError::BadRequest(error.to_string()));
        }
        return Ok(value.get("result").cloned().unwrap_or(value));
    }
    Err(AppError::Process(
        "MCP response wait exceeded the maximum number of frames".to_string(),
    ))
}

pub(super) async fn read_stdio_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    protocol: StdioProtocol,
) -> AppResult<Value> {
    match protocol {
        StdioProtocol::ContentLength => read_content_length_message(reader).await,
        StdioProtocol::JsonLines => read_json_lines_message(reader).await,
    }
}

pub(super) async fn read_content_length_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> AppResult<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(AppError::Process(
                "MCP process closed before response".to_string(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if let Some(length) = content_length {
                return read_stdio_payload(reader, length).await;
            }
            continue;
        }
        if let Some(length) = parse_content_length(trimmed)? {
            content_length = Some(length);
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(value);
        }
    }
}

pub(super) async fn read_json_lines_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> AppResult<Value> {
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(AppError::Process(
                "MCP process closed before response".to_string(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(value);
        }
        if let Some(length) = parse_content_length(trimmed)? {
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).await? == 0 {
                    return Err(AppError::Process(
                        "MCP process closed while reading stdio frame headers".to_string(),
                    ));
                }
                if header.trim().is_empty() {
                    return read_stdio_payload(reader, length).await;
                }
            }
        }
    }
}

pub(super) fn parse_content_length(line: &str) -> AppResult<Option<usize>> {
    let lower = line.to_ascii_lowercase();
    let Some(raw) = lower.strip_prefix("content-length:") else {
        return Ok(None);
    };
    let length = raw
        .trim()
        .parse::<usize>()
        .map_err(|_| AppError::Process(format!("invalid Content-Length header: {line}")))?;
    if length > MAX_STDIO_FRAME_BYTES {
        return Err(AppError::Process(format!(
            "stdio frame exceeds {} bytes",
            MAX_STDIO_FRAME_BYTES
        )));
    }
    Ok(Some(length))
}

pub(super) async fn read_stdio_payload(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    length: usize,
) -> AppResult<Value> {
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(AppError::from)
}
