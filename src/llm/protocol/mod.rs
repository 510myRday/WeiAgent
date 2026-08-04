pub mod anthropic;
pub(crate) mod common;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicUsize, Ordering},
        sync::{Arc, Mutex},
    };

    use axum::{
        Router,
        body::{Body, Bytes},
        extract::State,
        http::{HeaderMap, Method, Uri},
        response::Response,
        routing::any,
    };
    use chrono::Utc;
    use futures_util::StreamExt;
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        anthropic::AnthropicAdapter, common::ProtocolClient, gemini::GeminiAdapter,
        openai_chat::OpenAiChatAdapter, openai_responses::OpenAiResponsesAdapter,
    };
    use crate::{
        domain::{Agent, ChatMessage, ChatMessageStatus, ChatRole, ModelProvider, ProviderType},
        llm::{
            client::{DynChatClient, RequestOptions},
            types::StreamEvent,
        },
    };

    #[derive(Clone, Copy)]
    enum ProtocolKind {
        OpenAiResponses,
        Anthropic,
        Gemini,
    }

    #[derive(Clone)]
    struct UpstreamState {
        protocol: ProtocolKind,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    #[derive(Debug)]
    struct CapturedRequest {
        method: Method,
        path: String,
        headers: HeaderMap,
        body: Value,
    }

    fn fixture() -> (Agent, Vec<ChatMessage>) {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            description: "Writes code".to_string(),
            model_provider_id: Uuid::new_v4(),
            model_id: "test-model".to_string(),
            is_builtin: false,
            avatar_icon: "bot".to_string(),
            avatar_color: "#22c55e".to_string(),
            created_at: now,
            updated_at: now,
        };
        let conversation_id = Uuid::new_v4();
        let messages = vec![ChatMessage {
            id: Uuid::new_v4(),
            agent_id: agent.id,
            conversation_id,
            role: ChatRole::User,
            content: "run it".to_string(),
            reasoning_content: None,
            openai_response_items: Vec::new(),
            status: ChatMessageStatus::Done,
            created_at: now,
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
            usage: None,
        }];
        (agent, messages)
    }

    fn provider(provider_type: ProviderType, base_url: impl Into<String>) -> ModelProvider {
        ModelProvider {
            id: Uuid::new_v4(),
            provider_type,
            name: "test".to_string(),
            base_url: base_url.into(),
            api_keys: vec!["key".to_string()],
            key_strategy: Default::default(),
            models: vec!["model".to_string()],
            model_configs: Vec::new(),
            extra_body: Value::Null,
            prompt_cache: Default::default(),
            cache_capabilities: None,
            proxy_url: None,
            image_generation_url: None,
            image_edit_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    async fn test_upstream(
        State(state): State<UpstreamState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let body_json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        state.requests.lock().unwrap().push(CapturedRequest {
            method: method.clone(),
            path: uri.path().to_string(),
            headers,
            body: body_json.clone(),
        });

        if method == Method::GET {
            let payload = match state.protocol {
                ProtocolKind::OpenAiResponses | ProtocolKind::Anthropic => {
                    json!({"data": [{"id": "native-model"}]})
                }
                ProtocolKind::Gemini => json!({
                    "models": [{
                        "name": "models/native-model",
                        "supportedGenerationMethods": ["generateContent"]
                    }]
                }),
            };
            return json_response(payload);
        }

        let streaming =
            body_json["stream"] == true || uri.path().ends_with(":streamGenerateContent");
        if streaming {
            let payload = match state.protocol {
                ProtocolKind::OpenAiResponses => concat!(
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"streamed\"}\n\n",
                    "data: {\"type\":\"response.completed\"}\n\n"
                ),
                ProtocolKind::Anthropic => concat!(
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"streamed\"}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n"
                ),
                ProtocolKind::Gemini => {
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"streamed\"}]},\"finishReason\":\"STOP\"}]}\n\n"
                }
            };
            return Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .body(Body::from(payload))
                .unwrap();
        }

        let payload = match state.protocol {
            ProtocolKind::OpenAiResponses => json!({
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": "complete"}]
                }]
            }),
            ProtocolKind::Anthropic => {
                json!({"content": [{"type": "text", "text": "complete"}]})
            }
            ProtocolKind::Gemini => json!({
                "candidates": [{"content": {"parts": [{"text": "complete"}]}}]
            }),
        };
        json_response(payload)
    }

    fn json_response(value: Value) -> Response {
        Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_http_protocol_case(
        protocol: ProtocolKind,
        client: DynChatClient,
        provider_type: ProviderType,
        expected_complete_path: &str,
        expected_stream_path: &str,
        auth_header: &str,
        auth_value: &str,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = UpstreamState {
            protocol,
            requests: requests.clone(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(any(test_upstream)).with_state(state),
            )
            .await
            .unwrap();
        });

        let (agent, messages) = fixture();
        let provider = provider(provider_type, format!("http://{address}/v1"));
        let options = RequestOptions {
            timeout: std::time::Duration::from_secs(5),
            retry_attempts: 1,
            thinking_level: None,
            on_retry: None,
        };
        let complete = client
            .complete(
                &agent,
                &provider,
                &messages,
                "system",
                options.clone(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(complete.content, "complete");

        let mut stream = client
            .stream(&agent, &provider, &messages, "system", options, None)
            .await
            .unwrap();
        let mut streamed = String::new();
        while let Some(event) = stream.next().await {
            if let StreamEvent::TextDelta(delta) = event.unwrap() {
                streamed.push_str(&delta);
            }
        }
        assert_eq!(streamed, "streamed");
        assert_eq!(
            client.list_models(&provider).await.unwrap(),
            ["native-model"]
        );

        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0].method, Method::POST);
        assert_eq!(captured[0].path, expected_complete_path);
        assert_eq!(captured[1].path, expected_stream_path);
        assert_eq!(captured[2].method, Method::GET);
        assert_eq!(captured[2].path, "/v1/models");
        for request in captured.iter() {
            assert_eq!(
                request.headers.get(auth_header).unwrap().to_str().unwrap(),
                auth_value
            );
        }
        if matches!(protocol, ProtocolKind::Anthropic) {
            for request in captured.iter() {
                assert_eq!(
                    request
                        .headers
                        .get("anthropic-version")
                        .unwrap()
                        .to_str()
                        .unwrap(),
                    "2023-06-01"
                );
            }
        }
        if matches!(protocol, ProtocolKind::OpenAiResponses) {
            for request in &captured[..2] {
                assert_eq!(
                    request.body["include"],
                    json!(["reasoning.encrypted_content"])
                );
                assert_eq!(
                    request.body["prompt_cache_key"],
                    messages[0].conversation_id.to_string()
                );
                assert_eq!(request.body["store"], false);
                assert_eq!(request.body["tool_choice"], "auto");
                assert_eq!(request.body["parallel_tool_calls"], true);
                assert_eq!(request.body["input"][0]["type"], "message");
                assert_eq!(request.body["input"][0]["content"][0]["type"], "input_text");
            }
        }
        assert!(!captured[0].body.is_null());
        server.abort();
    }

    #[tokio::test]
    async fn clients_send_protocol_auth_endpoints_and_sse() {
        run_http_protocol_case(
            ProtocolKind::OpenAiResponses,
            Arc::new(ProtocolClient::new(OpenAiResponsesAdapter)),
            ProviderType::OpenAiResponses,
            "/v1/responses",
            "/v1/responses",
            "authorization",
            "Bearer key",
        )
        .await;
        run_http_protocol_case(
            ProtocolKind::Anthropic,
            Arc::new(ProtocolClient::new(AnthropicAdapter)),
            ProviderType::AnthropicMessages,
            "/v1/messages",
            "/v1/messages",
            "x-api-key",
            "key",
        )
        .await;
        run_http_protocol_case(
            ProtocolKind::Gemini,
            Arc::new(ProtocolClient::new(GeminiAdapter)),
            ProviderType::GoogleGemini,
            "/v1/models/test-model:generateContent",
            "/v1/models/test-model:streamGenerateContent",
            "x-goog-api-key",
            "key",
        )
        .await;
    }

    async fn idle_then_success_upstream(
        State(requests): State<Arc<AtomicUsize>>,
        method: Method,
    ) -> Response {
        if method != Method::POST {
            return Response::builder().status(404).body(Body::empty()).unwrap();
        }
        if requests.fetch_add(1, Ordering::SeqCst) == 0 {
            let body = async_stream::stream! {
                yield Ok::<Bytes, Infallible>(Bytes::from("\n"));
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                yield Ok::<Bytes, Infallible>(Bytes::from(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"stale\"}}]}\n\n",
                ));
            };
            return Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(body))
                .unwrap();
        }
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn streaming_idle_timeout_retries_before_first_event() {
        let requests = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_requests = requests.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .fallback(any(idle_then_success_upstream))
                    .with_state(server_requests),
            )
            .await
            .unwrap();
        });

        let (agent, messages) = fixture();
        let provider = provider(
            ProviderType::OpenAiCompatible,
            format!("http://{address}/v1"),
        );
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let retry_reasons = reasons.clone();
        let options = RequestOptions {
            timeout: std::time::Duration::from_millis(20),
            retry_attempts: 2,
            thinking_level: None,
            on_retry: Some(Arc::new(move |_attempt, _max, reason| {
                retry_reasons.lock().unwrap().push(reason.to_string());
            })),
        };
        let client: DynChatClient = Arc::new(ProtocolClient::new(OpenAiChatAdapter));
        let mut stream = client
            .stream(&agent, &provider, &messages, "system", options, None)
            .await
            .unwrap();
        let mut output = String::new();
        while let Some(event) = stream.next().await {
            if let StreamEvent::TextDelta(delta) = event.unwrap() {
                output.push_str(&delta);
            }
        }

        assert_eq!(output, "ok");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        let reasons = reasons.lock().unwrap();
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("LLM stream idle timeout"));
        server.abort();
    }

    #[test]
    fn provider_type_defaults_and_round_trips_new_modes() {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            provider_type: ProviderType,
        }

        let defaulted: Wrapper = serde_json::from_str("{}").unwrap();
        assert_eq!(defaulted.provider_type, ProviderType::OpenAiCompatible);
        for value in ["openai_responses", "anthropic_messages", "google_gemini"] {
            let parsed: ProviderType =
                serde_json::from_value(Value::String(value.to_string())).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        }
    }
}
