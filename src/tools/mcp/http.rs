// Internal MCP transport/auth helpers split out of the manager module.
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{Duration, timeout};

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::fs;

use super::auth::McpAuthRuntime;
use super::env::expand_mcp_value;
use super::protocol::{
    MAX_HTTP_BODY_BYTES, MAX_SSE_EVENT_BYTES, MCP_PROTOCOL_VERSION, McpHandshake, McpRpcResponse,
    mcp_initialize_request, parse_mcp_handshake, parse_rpc_response, parse_rpc_value,
    truncate_error_body,
};
use crate::{
    domain::{McpServer, McpTransport},
    error::{AppError, AppResult},
};

pub(super) async fn rpc_streamable_http(
    auth: &Arc<McpAuthRuntime>,
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<McpRpcResponse> {
    let (url, configured_token, headers, client_cert, client_key, identity_header, timeout_ms) =
        match &server.transport {
            McpTransport::StreamableHttp {
                url,
                bearer_token,
                headers,
                client_cert,
                client_key,
                identity_header,
                timeout_ms,
                ..
            } => (
                url,
                bearer_token.as_deref(),
                headers,
                client_cert.as_deref(),
                client_key.as_deref(),
                identity_header.as_ref(),
                *timeout_ms,
            ),
            _ => unreachable!(),
        };
    if url.trim().is_empty() {
        return Err(AppError::BadRequest("MCP HTTP URL is required".to_string()));
    }
    let client = build_http_client(timeout_ms, client_cert, client_key).await?;
    let oauth_token = auth.oauth_access_token(server).await;
    let bearer_token = configured_token.or(oauth_token.as_deref());
    let mut session: Option<String> = None;
    let mut protocol_version = MCP_PROTOCOL_VERSION.to_string();
    let mut handshake = McpHandshake::default();
    let result = async {
        let init = http_request(
            &client,
            url,
            bearer_token,
            headers,
            session.as_deref(),
            mcp_initialize_request(),
            &protocol_version,
            identity_header,
            Some(1),
        )
        .await?;
        session = init.1;
        handshake = parse_mcp_handshake(&init.0);
        if let Some(version) = init.0.get("protocolVersion").and_then(Value::as_str) {
            protocol_version = version.to_string();
        }
        let _ = http_request(
            &client,
            url,
            bearer_token,
            headers,
            session.as_deref(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            &protocol_version,
            identity_header,
            None,
        )
        .await?;
        if let Some((method, params)) = request {
            let (value, _) = http_request(
                &client,
                url,
                bearer_token,
                headers,
                session.as_deref(),
                json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}),
                &protocol_version,
                identity_header,
                Some(2),
            )
            .await?;
            Ok(value)
        } else {
            Ok(json!({}))
        }
    }
    .await;
    if let Some(session_id) = session.as_deref() {
        let _ = client
            .delete(url)
            .headers(build_headers(bearer_token, headers, identity_header, None)?)
            .header("MCP-Session-Id", session_id)
            .header("MCP-Protocol-Version", &protocol_version)
            .send()
            .await;
    }
    result.map(|value| McpRpcResponse {
        value,
        handshake: Some(handshake),
    })
}

pub(super) async fn rpc_sse(
    auth: &Arc<McpAuthRuntime>,
    server: &McpServer,
    request: Option<(&str, Value)>,
) -> AppResult<McpRpcResponse> {
    let McpTransport::Sse {
        url,
        bearer_token,
        headers,
        client_cert,
        client_key,
        identity_header,
        timeout_ms,
        ..
    } = &server.transport
    else {
        unreachable!()
    };
    if url.trim().is_empty() {
        return Err(AppError::BadRequest("MCP SSE URL is required".to_string()));
    }
    let oauth_token = auth.oauth_access_token(server).await;
    let bearer_token = bearer_token.as_deref().or(oauth_token.as_deref());
    let timeout_duration = Duration::from_millis((*timeout_ms).clamp(1_000, 120_000));
    let client =
        build_http_client(*timeout_ms, client_cert.as_deref(), client_key.as_deref()).await?;
    let mut stream_response = client
        .get(url)
        .headers(build_headers(
            bearer_token.as_deref(),
            headers,
            identity_header.as_ref(),
            Some("text/event-stream"),
        )?)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    if !stream_response.status().is_success() {
        return Err(AppError::Llm(format!(
            "MCP SSE endpoint returned {}",
            stream_response.status()
        )));
    }
    let endpoint_data = read_sse_event(&mut stream_response, timeout_duration).await?;
    let base_url =
        reqwest::Url::parse(url).map_err(|error| AppError::BadRequest(error.to_string()))?;
    let endpoint_url = resolve_sse_message_endpoint(&base_url, &endpoint_data.0, &endpoint_data.1)?;

    let mut protocol_version = MCP_PROTOCOL_VERSION.to_string();
    let initialize = mcp_initialize_request();
    let initialize_response = post_sse_message(
        &client,
        &endpoint_url,
        bearer_token.as_deref(),
        headers,
        identity_header.as_ref(),
        initialize,
        &protocol_version,
    )
    .await?;
    let initialize_result = if initialize_response.trim().is_empty() {
        read_sse_response(&mut stream_response, timeout_duration, 1).await?
    } else {
        parse_rpc_response(&initialize_response)?
    };
    let handshake = parse_mcp_handshake(&initialize_result);
    if let Some(version) = initialize_result
        .get("protocolVersion")
        .and_then(Value::as_str)
    {
        protocol_version = version.to_string();
    }

    let _ = post_sse_message(
        &client,
        &endpoint_url,
        bearer_token.as_deref(),
        headers,
        identity_header.as_ref(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        &protocol_version,
    )
    .await?;
    let result = if let Some((method, params)) = request {
        let response = post_sse_message(
            &client,
            &endpoint_url,
            bearer_token.as_deref(),
            headers,
            identity_header.as_ref(),
            json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}),
            &protocol_version,
        )
        .await?;
        if response.trim().is_empty() {
            read_sse_response(&mut stream_response, timeout_duration, 2).await
        } else {
            parse_rpc_response(&response)
        }
    } else {
        Ok(json!({}))
    };
    result.map(|value| McpRpcResponse {
        value,
        handshake: Some(handshake),
    })
}

pub(super) async fn http_request(
    client: &reqwest::Client,
    url: &str,
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    session: Option<&str>,
    body: Value,
    protocol_version: &str,
    identity_header: Option<&crate::domain::McpIdentityHeader>,
    expected_id: Option<i64>,
) -> AppResult<(Value, Option<String>)> {
    let mut header_map = build_headers(
        bearer_token,
        headers,
        identity_header,
        Some("application/json, text/event-stream"),
    )?;
    if let Some(session) = session {
        if let Ok(value) = HeaderValue::from_str(session) {
            header_map.insert("mcp-session-id", value);
        }
    }
    let response = client
        .post(url)
        .headers(header_map)
        .header("MCP-Protocol-Version", protocol_version)
        .json(&body)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = read_limited_body(response, MAX_HTTP_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(AppError::Llm(format!(
            "MCP HTTP endpoint returned {status}: {}",
            truncate_error_body(&text)
        )));
    }
    if text.trim().is_empty() {
        return Ok((json!({}), session));
    }
    let value = parse_http_mcp_value(&text, &content_type, expected_id)?;
    if let Some(expected_id) = expected_id {
        if value.get("id").and_then(Value::as_i64) != Some(expected_id)
            && value.get("result").is_none()
        {
            return Err(AppError::Llm(format!(
                "MCP response id mismatch: expected {expected_id}"
            )));
        }
    }
    Ok((parse_rpc_value(value)?, session))
}

pub(super) async fn build_http_client(
    timeout_ms: u64,
    client_cert: Option<&str>,
    client_key: Option<&str>,
) -> AppResult<reqwest::Client> {
    let mut config = crate::http_client::HttpClientConfig::new()
        .timeout(Duration::from_millis(timeout_ms.clamp(1_000, 120_000)));
    if let Some(cert_path) = client_cert.map(expand_mcp_value) {
        let cert = fs::read(PathBuf::from(cert_path)).await.map_err(|error| {
            AppError::BadRequest(format!("failed to read MCP client certificate: {error}"))
        })?;
        let identity_bytes = if let Some(key_path) = client_key.map(expand_mcp_value) {
            let key = fs::read(PathBuf::from(key_path)).await.map_err(|error| {
                AppError::BadRequest(format!("failed to read MCP client key: {error}"))
            })?;
            let mut combined = cert;
            combined.extend_from_slice(b"\n");
            combined.extend_from_slice(&key);
            combined
        } else {
            cert
        };
        let identity = reqwest::Identity::from_pem(&identity_bytes).map_err(|error| {
            AppError::BadRequest(format!("invalid MCP client certificate/key: {error}"))
        })?;
        config = config.identity(identity);
    } else if client_key.is_some() {
        return Err(AppError::BadRequest(
            "MCP client_key requires client_cert".to_string(),
        ));
    }
    crate::http_client::build_reqwest_client(config).map_err(AppError::Llm)
}

pub(super) async fn read_limited_body(
    response: reqwest::Response,
    limit: usize,
) -> AppResult<String> {
    if let Some(length) = response.content_length() {
        if length > limit as u64 {
            return Err(AppError::Llm(format!("MCP response exceeds {limit} bytes")));
        }
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| AppError::Llm(error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(AppError::Llm(format!("MCP response exceeds {limit} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|error| AppError::Llm(error.to_string()))
}

pub(super) fn parse_sse_response(text: &str, expected_id: Option<i64>) -> AppResult<Value> {
    for event in text.split("\n\n").chain(text.split("\r\n\r\n")) {
        if let Some((_, data)) = parse_sse_event(event) {
            if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                if expected_id.is_none() || value.get("id").and_then(Value::as_i64) == expected_id {
                    return Ok(value);
                }
            }
        }
    }
    Err(AppError::Llm(
        "MCP SSE response did not contain a matching JSON-RPC event".to_string(),
    ))
}

pub(super) fn parse_http_mcp_value(
    text: &str,
    content_type: &str,
    expected_id: Option<i64>,
) -> AppResult<Value> {
    let looks_like_sse = content_type.contains("text/event-stream")
        || (expected_id.is_some() && text.contains("data:"));
    if looks_like_sse {
        if let Ok(value) = parse_sse_response(text, expected_id) {
            return Ok(value);
        }
    }
    serde_json::from_str::<Value>(text)
        .or_else(|_| parse_sse_json(text))
        .map_err(AppError::Json)
}

pub(super) fn resolve_sse_message_endpoint(
    base_url: &reqwest::Url,
    event: &str,
    data: &str,
) -> AppResult<reqwest::Url> {
    let data = data.trim();
    if event.eq_ignore_ascii_case("ready") {
        return Ok(base_url.clone());
    }

    if let Ok(value) = serde_json::from_str::<Value>(data) {
        if let Some(endpoint) = value.get("endpoint").and_then(Value::as_str) {
            return resolve_sse_endpoint_url(base_url, endpoint);
        }
        if value.is_object() {
            return Ok(base_url.clone());
        }
        if let Some(endpoint) = value.as_str() {
            return resolve_sse_endpoint_url(base_url, endpoint);
        }
    }

    resolve_sse_endpoint_url(base_url, data)
}

pub(super) fn resolve_sse_endpoint_url(
    base_url: &reqwest::Url,
    endpoint: &str,
) -> AppResult<reqwest::Url> {
    base_url
        .join(endpoint.trim())
        .or_else(|_| reqwest::Url::parse(endpoint.trim()))
        .map_err(|error| AppError::BadRequest(format!("Invalid MCP SSE message endpoint: {error}")))
}

pub(super) fn build_headers(
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    identity_header: Option<&crate::domain::McpIdentityHeader>,
    accept: Option<&str>,
) -> AppResult<HeaderMap> {
    let mut header_map = HeaderMap::new();
    for (key, value) in headers {
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP header name: {error}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP header value: {error}")))?;
        header_map.insert(name, value);
    }
    let has_authorization = headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"));
    if !has_authorization
        && let Some(token) =
            bearer_token.filter(|token| !token.is_empty() && *token != "__preserve__")
    {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP bearer token: {error}")))?;
        header_map.insert("authorization", value);
    }
    if let Some(identity) = identity_header.filter(|item| !item.name.trim().is_empty()) {
        let name = HeaderName::from_bytes(identity.name.trim().as_bytes()).map_err(|error| {
            AppError::BadRequest(format!("Invalid MCP identity header name: {error}"))
        })?;
        let value = HeaderValue::from_str(&expand_mcp_value(&identity.value)).map_err(|error| {
            AppError::BadRequest(format!("Invalid MCP identity header value: {error}"))
        })?;
        if !header_map.keys().any(|key| key == &name) {
            header_map.insert(name, value);
        }
    }
    if let Some(accept) = accept {
        let value = HeaderValue::from_str(accept)
            .map_err(|error| AppError::BadRequest(format!("Invalid MCP accept header: {error}")))?;
        header_map.insert("accept", value);
    }
    Ok(header_map)
}

pub(super) async fn post_sse_message(
    client: &reqwest::Client,
    endpoint: &reqwest::Url,
    bearer_token: Option<&str>,
    headers: &std::collections::BTreeMap<String, String>,
    identity_header: Option<&crate::domain::McpIdentityHeader>,
    body: Value,
    protocol_version: &str,
) -> AppResult<String> {
    let response = client
        .post(endpoint.clone())
        .headers(build_headers(
            bearer_token,
            headers,
            identity_header,
            Some("application/json, text/event-stream"),
        )?)
        .header("MCP-Protocol-Version", protocol_version)
        .json(&body)
        .send()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Llm(error.to_string()))?;
    if body.len() > MAX_HTTP_BODY_BYTES {
        return Err(AppError::Llm(format!(
            "MCP SSE response exceeds {MAX_HTTP_BODY_BYTES} bytes"
        )));
    }
    if !status.is_success() {
        return Err(AppError::Llm(format!(
            "MCP SSE message endpoint returned {status}: {body}"
        )));
    }
    Ok(body)
}

pub(super) async fn read_sse_event(
    response: &mut reqwest::Response,
    timeout_duration: Duration,
) -> AppResult<(String, String)> {
    let mut buffer = Vec::new();
    loop {
        if let Some((index, boundary_len)) = find_sse_boundary(&buffer) {
            let event = buffer.drain(..index).collect::<Vec<_>>();
            buffer.drain(..boundary_len);
            if let Some(parsed) = parse_sse_event(&String::from_utf8_lossy(&event)) {
                return Ok(parsed);
            }
            continue;
        }
        let chunk = timeout(timeout_duration, response.chunk())
            .await
            .map_err(|_| AppError::Llm("MCP SSE response timed out".to_string()))?
            .map_err(|error| AppError::Llm(error.to_string()))?;
        let Some(chunk) = chunk else {
            return Err(AppError::Llm(
                "MCP SSE stream closed before a response".to_string(),
            ));
        };
        if buffer.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BYTES {
            return Err(AppError::Llm(format!(
                "MCP SSE event exceeds {MAX_SSE_EVENT_BYTES} bytes"
            )));
        }
        buffer.extend_from_slice(&chunk);
    }
}

pub(super) async fn read_sse_response(
    response: &mut reqwest::Response,
    timeout_duration: Duration,
    expected_id: i64,
) -> AppResult<Value> {
    loop {
        let (_, data) = read_sse_event(response, timeout_duration).await?;
        let Ok(value) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        return parse_rpc_value(value);
    }
}

pub(super) fn find_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    match (crlf, lf) {
        (Some(crlf), Some(lf)) if crlf < lf => Some((crlf, 4)),
        (Some(crlf), _) => Some((crlf, 4)),
        (_, Some(lf)) => Some((lf, 2)),
        _ => None,
    }
}

pub(super) fn parse_sse_event(text: &str) -> Option<(String, String)> {
    let mut event = String::new();
    let mut data = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_string());
        }
    }
    (!data.is_empty()).then(|| (event, data.join("\n")))
}

pub(super) fn parse_sse_json(text: &str) -> Result<Value, serde_json::Error> {
    for line in text.lines().filter_map(|line| line.strip_prefix("data:")) {
        if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
            return Ok(value);
        }
    }
    serde_json::from_str(text)
}
