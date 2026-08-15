// Internal MCP transport/auth helpers split out of the manager module.
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::{
    domain::{McpServer, McpServerStatus, McpTransport},
    error::{AppError, AppResult},
};

pub(super) const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
pub(super) const MAX_TOOL_NAME_BYTES: usize = 64;
pub(super) const STDIO_IDLE_TTL: Duration = Duration::from_secs(300);
pub(super) const STDIO_MAX_RETRIES: u32 = 2;
pub(super) const STDIO_MAX_RESPONSE_WAIT_ITERATIONS: usize = 100;
pub(super) const MAX_STDIO_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_TOOLS_LIST_PAGES: usize = 100;
pub(super) const MAX_MCP_STDERR_ERROR_CHARS: usize = 4_000;

pub(super) fn mcp_initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "WeiAgent",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

pub(super) fn is_mcp_remote(server: &McpServer) -> bool {
    let McpTransport::Stdio { command, args, .. } = &server.transport else {
        return false;
    };
    let command_name = PathBuf::from(command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    command_name == "mcp-remote"
        || args.iter().any(|arg| {
            arg.to_ascii_lowercase()
                .split('@')
                .next()
                .unwrap_or_default()
                == "mcp-remote"
        })
}

#[derive(Debug, Clone, Default)]
pub(super) struct McpHandshake {
    pub(super) protocol_version: Option<String>,
    pub(super) server_name: Option<String>,
    pub(super) server_version: Option<String>,
    pub(super) capabilities: Value,
}

#[derive(Debug)]
pub(super) struct McpRpcResponse {
    pub(super) value: Value,
    pub(super) handshake: Option<McpHandshake>,
}

pub(super) fn parse_mcp_handshake(result: &Value) -> McpHandshake {
    let server_info = result.get("serverInfo");
    McpHandshake {
        protocol_version: result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .map(str::to_string),
        server_name: server_info
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        server_version: server_info
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .map(str::to_string),
        capabilities: result.get("capabilities").cloned().unwrap_or(Value::Null),
    }
}

pub(super) fn status_name(status: McpServerStatus) -> &'static str {
    match status {
        McpServerStatus::Unknown => "unknown",
        McpServerStatus::Connecting => "connecting",
        McpServerStatus::Ready => "ready",
        McpServerStatus::Offline => "offline",
        McpServerStatus::Error => "error",
    }
}

pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp_") || name.starts_with("mcp__")
}

pub(super) struct McpCallOutput {
    pub(super) text: String,
    pub(super) content: Value,
    pub(super) is_error: bool,
}

pub(super) fn normalize_mcp_input_schema(value: Option<&Value>) -> Value {
    match value {
        Some(schema) if schema.is_object() => schema.clone(),
        _ => json!({"type": "object", "properties": {}, "additionalProperties": true}),
    }
}

pub(super) fn mcp_timeout_ms(transport: &McpTransport) -> u64 {
    match transport {
        McpTransport::Stdio {
            command,
            timeout_ms,
            ..
        } => {
            // The first run of npx/uv may install a package before the MCP
            // process starts speaking JSON-RPC.
            let package_manager = command
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(command)
                .trim_end_matches(".cmd")
                .trim_end_matches(".exe")
                .to_ascii_lowercase();
            if matches!(package_manager.as_str(), "npx" | "npm" | "uv" | "uvx") {
                (*timeout_ms).max(120_000)
            } else {
                *timeout_ms
            }
        }
        McpTransport::StreamableHttp { timeout_ms, .. } | McpTransport::Sse { timeout_ms, .. } => {
            *timeout_ms
        }
    }
}

pub(super) fn mcp_timeout_error(timeout_duration: Duration) -> AppError {
    AppError::Process(format!(
        "MCP stdio request timed out after {}ms",
        timeout_duration.as_millis()
    ))
}

pub(super) fn should_try_stdio_protocol_fallback(error: &AppError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("timed out")
        || message.contains("closed before response")
        || message.contains("content-length")
        || message.contains("stdio frame")
}

pub(super) fn is_retryable_stdio_error(error: &AppError) -> bool {
    matches!(error, AppError::Process(_) | AppError::Json(_))
}

pub(super) fn truncate_error_body(body: &str) -> String {
    crate::textutil::truncate_chars(body, 512)
}

pub(super) fn format_mcp_stderr(lines: &[String]) -> Option<String> {
    let output = lines.join("\n");
    if output.is_empty() {
        return None;
    }
    Some(crate::textutil::truncate_chars(
        &output,
        MAX_MCP_STDERR_ERROR_CHARS,
    ))
}

pub(super) fn parse_rpc_response(text: &str) -> AppResult<Value> {
    parse_rpc_value(serde_json::from_str(text)?)
}

pub(super) fn parse_rpc_value(value: Value) -> AppResult<Value> {
    if let Some(error) = value.get("error") {
        return Err(AppError::BadRequest(error.to_string()));
    }
    Ok(value.get("result").cloned().unwrap_or(value))
}
