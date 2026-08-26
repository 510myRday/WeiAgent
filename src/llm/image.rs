use std::{
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::ImageDecoder;
use reqwest::multipart;
use serde_json::{Map, Value, json};

use crate::{
    domain::{
        ImageAttachment, ImageModelConfig, ImageSource, ModelProvider, ProviderType, TokenUsage,
    },
    error::{AppError, AppResult},
    llm::{
        client::RequestOptions,
        http::{
            DEFAULT_MODEL_LIST_TIMEOUT, LlmHttp, add_openrouter_attribution, log_request,
            read_response_body_limited, read_response_body_limited_to,
        },
        keys::ApiKeyCycle,
    },
};

const MAX_IMAGE_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
const MAX_GENERATED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_GENERATED_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const MAX_REFERENCE_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_REFERENCE_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const REMOTE_DNS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
}

#[derive(Debug, Clone)]
pub struct GeneratedImages {
    pub images: Vec<GeneratedImage>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug)]
struct ReferenceImage {
    name: String,
    media_type: String,
    bytes: Vec<u8>,
    is_mask: bool,
    dimensions: (u32, u32),
}

#[derive(Debug, Clone, Default)]
pub struct ImageRequestOptions {
    pub size: Option<String>,
    pub quality: Option<String>,
    pub n: Option<u32>,
    pub output_format: Option<String>,
    pub output_compression: Option<u32>,
    pub background: Option<String>,
    pub moderation: Option<String>,
    pub style: Option<String>,
    pub response_format: Option<String>,
    pub input_fidelity: Option<String>,
}

#[derive(Clone)]
pub struct ImageGenerationRequest {
    pub provider: ModelProvider,
    pub model: String,
    pub prompt: String,
    pub references: Vec<ImageAttachment>,
    pub options: RequestOptions,
    pub image_options: ImageRequestOptions,
    pub log_context: Option<ImageLogContext>,
}

struct ImageResponseContext<'a> {
    image_fetch_client: &'a reqwest::Client,
    options: &'a RequestOptions,
    log_context: Option<&'a ImageLogContext>,
    request_json: &'a str,
    endpoint: &'a str,
    operation: &'a str,
    started: Instant,
}

impl From<&ImageModelConfig> for ImageRequestOptions {
    fn from(config: &ImageModelConfig) -> Self {
        Self {
            size: config.size.clone(),
            quality: config.quality.clone(),
            n: config.n,
            output_format: config.output_format.clone(),
            output_compression: config.output_compression,
            background: config.background.clone(),
            moderation: config.moderation.clone(),
            style: config.style.clone(),
            response_format: config.response_format.clone(),
            input_fidelity: config.input_fidelity.clone(),
        }
    }
}

#[async_trait]
pub trait ImageClient: Send + Sync {
    async fn generate(&self, request: ImageGenerationRequest) -> AppResult<GeneratedImages>;

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>>;
}

pub type DynImageClient = std::sync::Arc<dyn ImageClient>;

#[derive(Debug, Clone, Default)]
pub struct ImageLogContext {
    pub agent_name: String,
    pub agent_id: Option<String>,
    pub conversation_id: Option<String>,
    pub model_id: String,
    pub provider_name: String,
}

#[derive(Clone)]
pub struct OpenAiImagesClient {
    http: LlmHttp,
}

impl OpenAiImagesClient {
    pub fn new(logger: Arc<crate::storage::LlmLogger>) -> Self {
        Self {
            http: LlmHttp::new().with_logger(logger),
        }
    }

    fn generation_url(provider: &ModelProvider) -> String {
        provider
            .image_generation_url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| default_image_endpoint(&provider.base_url, "generations"))
    }

    fn edit_url(provider: &ModelProvider) -> String {
        provider
            .image_edit_url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| default_image_endpoint(&provider.base_url, "edits"))
    }

    async fn response_images(
        &self,
        response: reqwest::Response,
        context: ImageResponseContext<'_>,
    ) -> AppResult<GeneratedImages> {
        let status = response.status();
        let mut body_for_log = None;
        let result = async {
            let body = read_response_body_limited_to(
                response,
                context.options.timeout,
                MAX_IMAGE_RESPONSE_BYTES,
                "image response",
            )
            .await?;
            let body = String::from_utf8(body).map_err(|error| {
                AppError::Llm(format!("image response was not valid UTF-8: {error}"))
            })?;
            if !status.is_success() {
                body_for_log = Some(log_excerpt(&body, 64 * 1024));
                if status == reqwest::StatusCode::NOT_FOUND {
                    let hint = if context.operation == "image edit" {
                        "The selected provider may expose image generation but not image editing; configure its image edit URL or choose a provider/model with image editing support"
                    } else {
                        "Check the provider Base URL or configure its image generation URL"
                    };
                    return Err(AppError::Llm(format!(
                        "{operation} endpoint returned 404: {endpoint}. {hint}. Upstream response: {body}",
                        operation = context.operation,
                        endpoint = context.endpoint,
                    )));
                }
                return Err(AppError::Llm(format!(
                    "image upstream returned {status} from {endpoint}: {body}",
                    endpoint = context.endpoint,
                )));
            }
            let value: Value = serde_json::from_str(&body)
                .map_err(|error| AppError::Llm(format!("invalid image upstream JSON: {error}")))?;
            body_for_log = Some(image_response_log_summary(&value));
            let usage = parse_image_usage(value.get("usage"));
            let items = value["data"].as_array().ok_or_else(|| {
                AppError::Llm("image upstream response is missing data".to_string())
            })?;
            let mut images = Vec::with_capacity(items.len());
            let mut total_bytes = 0_usize;
            for item in items {
                if let Some(encoded) = item["b64_json"].as_str() {
                    let bytes = STANDARD.decode(encoded).map_err(|error| {
                        AppError::Llm(format!("invalid generated image Base64: {error}"))
                    })?;
                    validate_image_size(
                        bytes.len(),
                        &mut total_bytes,
                        MAX_GENERATED_IMAGE_BYTES,
                        MAX_GENERATED_TOTAL_BYTES,
                        "generated image",
                    )?;
                    let media_type = detected_media_type(&bytes)?;
                    images.push(GeneratedImage { bytes, media_type });
                } else if let Some(url) = item["url"].as_str() {
                    let image = fetch_remote_image(
                        context.image_fetch_client,
                        url,
                        context.options.timeout,
                        MAX_GENERATED_IMAGE_BYTES,
                        "generated image",
                    )
                    .await?;
                    validate_image_size(
                        image.bytes.len(),
                        &mut total_bytes,
                        MAX_GENERATED_IMAGE_BYTES,
                        MAX_GENERATED_TOTAL_BYTES,
                        "generated image",
                    )?;
                    images.push(image);
                }
            }
            if images.is_empty() {
                return Err(AppError::Llm(
                    "image upstream returned no image data".to_string(),
                ));
            }
            Ok(GeneratedImages { images, usage })
        }
        .await;
        let error_message = result.as_ref().err().map(ToString::to_string);
        log_image_request(
            &self.http,
            context.log_context,
            context.request_json,
            body_for_log.as_deref(),
            Some(status.as_u16()),
            error_message.as_deref(),
            context.started,
        );
        result
    }

    async fn reference_image(
        &self,
        image_fetch_client: &reqwest::Client,
        attachment: &ImageAttachment,
        timeout: std::time::Duration,
    ) -> AppResult<GeneratedImage> {
        match &attachment.source {
            ImageSource::Base64 { data } => {
                let bytes = STANDARD.decode(data).map_err(|error| {
                    AppError::BadRequest(format!("invalid reference image: {error}"))
                })?;
                if bytes.len() > MAX_REFERENCE_IMAGE_BYTES {
                    return Err(AppError::BadRequest(format!(
                        "reference image exceeds {MAX_REFERENCE_IMAGE_BYTES} bytes"
                    )));
                }
                let media_type = detected_media_type(&bytes)?;
                Ok(GeneratedImage { bytes, media_type })
            }
            ImageSource::Url { url } => {
                fetch_remote_image(
                    image_fetch_client,
                    url,
                    timeout,
                    MAX_REFERENCE_IMAGE_BYTES,
                    "reference image",
                )
                .await
            }
            ImageSource::Local { .. } => Err(AppError::BadRequest(
                "local generated images cannot be sent as new references".to_string(),
            )),
        }
    }
}

#[async_trait]
impl ImageClient for OpenAiImagesClient {
    async fn generate(&self, request: ImageGenerationRequest) -> AppResult<GeneratedImages> {
        let ImageGenerationRequest {
            provider,
            model,
            prompt,
            references,
            options,
            image_options,
            log_context,
        } = request;
        if provider.provider_type != ProviderType::OpenAiImages {
            return Err(AppError::BadRequest(
                "selected provider is not an image provider".to_string(),
            ));
        }
        if prompt.trim().is_empty() {
            return Err(AppError::BadRequest(
                "image prompt cannot be empty".to_string(),
            ));
        }
        let client = self.http.client_for(provider.proxy_url.as_deref())?;
        let image_fetch_client = self
            .http
            .client_for_image_fetch(provider.proxy_url.as_deref())?;
        let mut keys = ApiKeyCycle::new(&provider)?;
        let image_options = effective_image_options(image_options);
        if references.is_empty() {
            let mut body = Map::new();
            body.insert("model".to_string(), json!(model));
            body.insert("prompt".to_string(), json!(prompt));
            if let Some(size) = image_options.size {
                body.insert("size".to_string(), json!(size));
            }
            if let Some(quality) = image_options.quality {
                body.insert("quality".to_string(), json!(quality));
            }
            if let Some(n) = image_options.n {
                body.insert("n".to_string(), json!(n));
            }
            if let Some(format) = image_options.output_format {
                body.insert("output_format".to_string(), json!(format));
            }
            if let Some(compression) = image_options.output_compression {
                body.insert("output_compression".to_string(), json!(compression));
            }
            if let Some(background) = image_options.background {
                body.insert("background".to_string(), json!(background));
            }
            if let Some(moderation) = image_options.moderation {
                body.insert("moderation".to_string(), json!(moderation));
            }
            if let Some(style) = image_options.style {
                body.insert("style".to_string(), json!(style));
            }
            if let Some(response_format) = image_options.response_format {
                body.insert("response_format".to_string(), json!(response_format));
            }
            if let Some(input_fidelity) = image_options.input_fidelity {
                body.insert("input_fidelity".to_string(), json!(input_fidelity));
            }
            if let Value::Object(extra) = &provider.extra_body {
                for (key, value) in extra {
                    if matches!(key.as_str(), "stream" | "partial_images") {
                        continue;
                    }
                    body.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            let body = Value::Object(body);
            let url = Self::generation_url(&provider);
            let request_json = serde_json::to_string_pretty(&body).unwrap_or_default();
            let started = Instant::now();
            let response = match self
                .http
                .retry_send(&options, &mut keys, |key| {
                    add_openrouter_attribution(
                        client.post(&url).bearer_auth(key).json(&body),
                        &provider,
                    )
                })
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    log_image_request(
                        &self.http,
                        log_context.as_ref(),
                        &request_json,
                        None,
                        None,
                        Some(&error.to_string()),
                        started,
                    );
                    return Err(error);
                }
            };
            return self
                .response_images(
                    response,
                    ImageResponseContext {
                        image_fetch_client: &image_fetch_client,
                        options: &options,
                        log_context: log_context.as_ref(),
                        request_json: &request_json,
                        endpoint: &url,
                        operation: "image generation",
                        started,
                    },
                )
                .await;
        }

        let mut reference_images = Vec::with_capacity(references.len());
        let mut total_reference_bytes = 0_usize;
        for reference in &references {
            let image = self
                .reference_image(&image_fetch_client, reference, options.timeout)
                .await?;
            total_reference_bytes = total_reference_bytes.saturating_add(image.bytes.len());
            if total_reference_bytes > MAX_REFERENCE_TOTAL_BYTES {
                return Err(AppError::BadRequest(format!(
                    "reference images exceed {MAX_REFERENCE_TOTAL_BYTES} total bytes"
                )));
            }
            let dimensions = image_dimensions(&image.bytes, "reference image")?;
            reference_images.push(ReferenceImage {
                name: reference.name.clone(),
                media_type: image.media_type,
                bytes: image.bytes,
                is_mask: reference.is_mask,
                dimensions,
            });
        }
        validate_mask_image(&reference_images)?;
        let url = Self::edit_url(&provider);
        let prompt = prompt.to_string();
        let model = model.to_string();
        let request_json = serde_json::to_string_pretty(&json!({
            "model": model,
            "prompt": prompt,
            "references": references.iter().map(|reference| json!({
                "name": reference.name,
                "media_type": reference.media_type,
                "is_mask": reference.is_mask,
            })).collect::<Vec<_>>(),
        }))
        .unwrap_or_default();
        let started = Instant::now();
        let response = match self
            .http
            .retry_send(&options, &mut keys, |key| {
                let mut form = multipart::Form::new()
                    .text("model", model.clone())
                    .text("prompt", prompt.clone());
                for reference in &reference_images {
                    let part = multipart::Part::bytes(reference.bytes.clone())
                        .file_name(reference.name.clone())
                        .mime_str(&reference.media_type)
                        .unwrap_or_else(|_| {
                            multipart::Part::bytes(reference.bytes.clone())
                                .file_name(reference.name.clone())
                        });
                    form = form.part(if reference.is_mask { "mask" } else { "image[]" }, part);
                }
                if let Some(size) = &image_options.size {
                    form = form.text("size", size.clone());
                }
                if let Some(quality) = &image_options.quality {
                    form = form.text("quality", quality.clone());
                }
                if let Some(n) = image_options.n {
                    form = form.text("n", n.to_string());
                }
                if let Some(format) = &image_options.output_format {
                    form = form.text("output_format", format.clone());
                }
                if let Some(compression) = image_options.output_compression {
                    form = form.text("output_compression", compression.to_string());
                }
                if let Some(background) = &image_options.background {
                    form = form.text("background", background.clone());
                }
                if let Some(moderation) = &image_options.moderation {
                    form = form.text("moderation", moderation.clone());
                }
                if let Some(style) = &image_options.style {
                    form = form.text("style", style.clone());
                }
                if let Some(response_format) = &image_options.response_format {
                    form = form.text("response_format", response_format.clone());
                }
                if let Some(input_fidelity) = &image_options.input_fidelity {
                    form = form.text("input_fidelity", input_fidelity.clone());
                }
                form = add_extra_form_fields(
                    form,
                    &provider.extra_body,
                    &[
                        "model",
                        "prompt",
                        "image",
                        "image[]",
                        "mask",
                        "stream",
                        "partial_images",
                    ],
                );
                add_openrouter_attribution(
                    client.post(&url).bearer_auth(key).multipart(form),
                    &provider,
                )
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                log_image_request(
                    &self.http,
                    log_context.as_ref(),
                    &request_json,
                    None,
                    None,
                    Some(&error.to_string()),
                    started,
                );
                return Err(error);
            }
        };
        self.response_images(
            response,
            ImageResponseContext {
                image_fetch_client: &image_fetch_client,
                options: &options,
                log_context: log_context.as_ref(),
                request_json: &request_json,
                endpoint: &url,
                operation: "image edit",
                started,
            },
        )
        .await
    }

    async fn list_models(&self, provider: &ModelProvider) -> AppResult<Vec<String>> {
        let client = self.http.client_for(provider.proxy_url.as_deref())?;
        let mut keys = ApiKeyCycle::new(provider)?;
        let options = RequestOptions {
            timeout: DEFAULT_MODEL_LIST_TIMEOUT,
            retry_attempts: 1,
            thinking_level: None,
            on_retry: None,
            is_compaction: false,
        };
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let response = self
            .http
            .retry_send(&options, &mut keys, |key| {
                add_openrouter_attribution(client.get(&url).bearer_auth(key), provider)
            })
            .await?;
        let status = response.status();
        let body = read_response_body_limited(response, options.timeout).await?;
        if !status.is_success() {
            return Err(AppError::Llm(format!(
                "image model list returned {status}: {body}"
            )));
        }
        let value: Value = serde_json::from_str(&body)?;
        Ok(value["data"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["id"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }
}

fn log_image_request(
    http: &LlmHttp,
    context: Option<&ImageLogContext>,
    request_json: &str,
    response: Option<&str>,
    status: Option<u16>,
    error: Option<&str>,
    started: Instant,
) {
    let Some(context) = context else { return };
    crate::llm::http::spawn_log(
        &http.logger(),
        log_request(
            crate::storage::llm_logs::LlmLogIdentity {
                agent_name: context.agent_name.clone(),
                agent_id: context.agent_id.clone(),
                conversation_id: context.conversation_id.clone(),
                is_compaction: false,
                model_id: context.model_id.clone(),
                provider_name: context.provider_name.clone(),
            },
            request_json,
            response,
            status,
            error,
            crate::storage::llm_logs::LlmLogMetrics::timed(started),
            http.log_enabled(),
        ),
    );
}

fn image_response_log_summary(value: &Value) -> String {
    let data = value["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    json!({
                        "base64_chars": item["b64_json"].as_str().map(str::len),
                        "has_url": item["url"].is_string(),
                        "revised_prompt": item.get("revised_prompt"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::to_string_pretty(&json!({
        "created": value.get("created"),
        "data": data,
        "usage": value.get("usage"),
    }))
    .unwrap_or_default()
}

fn parse_image_usage(value: Option<&Value>) -> Option<TokenUsage> {
    let value = value?.as_object()?;
    let number = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| value.get(*name).and_then(Value::as_u64))
            .unwrap_or_default()
    };
    let input_tokens = number(&["input_tokens", "prompt_tokens"]);
    let output_tokens = number(&["output_tokens", "completion_tokens"]);
    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }
    Some(
        TokenUsage {
            input_tokens,
            output_tokens,
            ..TokenUsage::default()
        }
        .normalized(),
    )
}

fn log_excerpt(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[image response log truncated]", &value[..end])
}

fn effective_image_options(mut options: ImageRequestOptions) -> ImageRequestOptions {
    options.n = options.n.filter(|value| (1..=10).contains(value));
    options.output_compression = options.output_compression.filter(|value| *value <= 100);
    options.output_format = allowed_value(options.output_format, &["png", "jpeg", "webp"]);
    options.background = allowed_value(options.background, &["auto", "opaque", "transparent"]);
    options.moderation = allowed_value(options.moderation, &["auto", "low"]);
    options.style = allowed_value(options.style, &["vivid", "natural"]);
    options.response_format = allowed_value(options.response_format, &["b64_json", "url"]);
    options.input_fidelity = allowed_value(options.input_fidelity, &["low", "high"]);
    options
}

fn allowed_value(value: Option<String>, allowed: &[&str]) -> Option<String> {
    value.filter(|value| allowed.contains(&value.as_str()))
}

async fn fetch_remote_image(
    client: &reqwest::Client,
    raw_url: &str,
    timeout: Duration,
    max_bytes: usize,
    label: &str,
) -> AppResult<GeneratedImage> {
    let url = validate_remote_image_url(raw_url, timeout).await?;
    let response = tokio::time::timeout(timeout, client.get(url.clone()).send())
        .await
        .map_err(|_| AppError::Llm(format!("{label} request timed out")))?
        .map_err(|error| AppError::Llm(format!("failed to fetch {label}: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::Llm(format!("{label} returned {status}")));
    }
    let bytes = read_response_body_limited_to(response, timeout, max_bytes, label).await?;
    let media_type = detected_media_type(&bytes)?;
    Ok(GeneratedImage { bytes, media_type })
}

pub(crate) async fn validate_remote_image_url(
    raw_url: &str,
    timeout: Duration,
) -> AppResult<reqwest::Url> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|_| AppError::BadRequest("image URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::BadRequest(
            "image URLs must use http or https".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::BadRequest(
            "image URLs cannot contain credentials".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppError::BadRequest("image URL is missing a host".to_string()))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(AppError::BadRequest(
            "image URL points to a local address".to_string(),
        ));
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        ensure_public_ip(address)?;
        return Ok(url);
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let dns_timeout = timeout.min(REMOTE_DNS_TIMEOUT);
    let addresses = tokio::time::timeout(dns_timeout, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| AppError::Llm("image URL DNS lookup timed out".to_string()))?
        .map_err(|error| AppError::Llm(format!("image URL DNS lookup failed: {error}")))?;
    let mut resolved = false;
    for address in addresses {
        resolved = true;
        ensure_public_ip(address.ip())?;
    }
    if !resolved {
        return Err(AppError::BadRequest(
            "image URL did not resolve to an address".to_string(),
        ));
    }
    Ok(url)
}

fn ensure_public_ip(address: IpAddr) -> AppResult<()> {
    let blocked = match address {
        IpAddr::V4(address) => is_blocked_ipv4(address),
        IpAddr::V6(address) => is_blocked_ipv6(address),
    };
    if blocked {
        return Err(AppError::BadRequest(
            "image URL points to a private or local address".to_string(),
        ));
    }
    Ok(())
}

fn is_blocked_ipv4(address: Ipv4Addr) -> bool {
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_unspecified()
        || address.is_multicast()
        || address.octets()[0] == 0
        || address.octets()[0] >= 240
        || address.octets() == [169, 254, 169, 254]
}

fn is_blocked_ipv6(address: Ipv6Addr) -> bool {
    address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (address.segments()[0] & 0xfe00) == 0xfc00
        || (address.segments()[0] & 0xffc0) == 0xfe80
        || address.to_ipv4_mapped().is_some_and(is_blocked_ipv4)
}

fn validate_image_size(
    bytes: usize,
    total: &mut usize,
    per_image_limit: usize,
    total_limit: usize,
    label: &str,
) -> AppResult<()> {
    if bytes > per_image_limit {
        return Err(AppError::Llm(format!(
            "{label} exceeded {per_image_limit} bytes"
        )));
    }
    *total = total.saturating_add(bytes);
    if *total > total_limit {
        return Err(AppError::Llm(format!(
            "generated images exceeded {total_limit} total bytes"
        )));
    }
    Ok(())
}

fn detected_media_type(bytes: &[u8]) -> AppResult<String> {
    crate::image_util::detect_image_media_type(bytes)
        .map(str::to_string)
        .ok_or_else(|| {
            AppError::Llm("image response did not contain a supported image".to_string())
        })
}

fn image_dimensions(bytes: &[u8], label: &str) -> AppResult<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| AppError::BadRequest(format!("invalid {label}: {error}")))?
        .into_dimensions()
        .map_err(|error| AppError::BadRequest(format!("invalid {label} dimensions: {error}")))
}

fn validate_mask_image(references: &[ReferenceImage]) -> AppResult<()> {
    let masks = references
        .iter()
        .filter(|reference| reference.is_mask)
        .collect::<Vec<_>>();
    if masks.len() > 1 {
        return Err(AppError::BadRequest(
            "a request can contain at most one mask image".to_string(),
        ));
    }
    let Some(mask) = masks.first() else {
        return Ok(());
    };
    let source = references
        .iter()
        .find(|reference| !reference.is_mask)
        .ok_or_else(|| {
            AppError::BadRequest("a mask image requires at least one source image".to_string())
        })?;
    if mask.media_type != "image/png" {
        return Err(AppError::BadRequest(
            "the mask image must contain PNG data".to_string(),
        ));
    }
    if mask.dimensions != source.dimensions {
        return Err(AppError::BadRequest(
            "the mask image dimensions must match the first source image".to_string(),
        ));
    }
    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&mask.bytes))
        .map_err(|error| AppError::BadRequest(format!("invalid PNG mask image: {error}")))?;
    if !decoder.color_type().has_alpha() {
        return Err(AppError::BadRequest(
            "the mask PNG must include an alpha channel".to_string(),
        ));
    }
    Ok(())
}

fn default_image_endpoint(base_url: &str, operation: &str) -> String {
    format!("{}/images/{operation}", base_url.trim_end_matches('/'))
}

fn add_extra_form_fields(
    mut form: multipart::Form,
    extra_body: &Value,
    reserved: &[&str],
) -> multipart::Form {
    let Value::Object(extra) = extra_body else {
        return form;
    };
    for (key, value) in extra {
        if reserved.contains(&key.as_str()) {
            continue;
        }
        let Some(value) = value_to_form_text(value) else {
            continue;
        };
        form = form.text(key.clone(), value);
    }
    form
}

fn value_to_form_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        _ => serde_json::to_string(value).ok(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ImageRequestOptions, ReferenceImage, default_image_endpoint, detected_media_type,
        effective_image_options, parse_image_usage, validate_mask_image, validate_remote_image_url,
    };

    #[test]
    fn default_image_endpoint_trims_trailing_slashes() {
        assert_eq!(
            default_image_endpoint("https://api.example.test/v1///", "edits"),
            "https://api.example.test/v1/images/edits"
        );
    }

    #[test]
    fn configured_image_options_are_preserved_without_model_specific_filtering() {
        let filtered = effective_image_options(ImageRequestOptions {
            response_format: Some("url".to_string()),
            input_fidelity: Some("high".to_string()),
            style: Some("vivid".to_string()),
            background: Some("transparent".to_string()),
            ..Default::default()
        });
        assert_eq!(filtered.response_format.as_deref(), Some("url"));
        assert_eq!(filtered.input_fidelity.as_deref(), Some("high"));
        assert_eq!(filtered.style.as_deref(), Some("vivid"));
        assert_eq!(filtered.background.as_deref(), Some("transparent"));
    }

    #[tokio::test]
    async fn rejects_private_and_non_http_image_urls() {
        assert!(
            validate_remote_image_url("http://127.0.0.1/image.png", Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(
            validate_remote_image_url("file:///tmp/image.png", Duration::from_secs(1))
                .await
                .is_err()
        );
    }

    #[test]
    fn rejects_invalid_image_bytes_and_parses_usage() {
        assert!(detected_media_type(b"not-an-image").is_err());
        let usage = parse_image_usage(Some(&serde_json::json!({
            "input_tokens": 12,
            "output_tokens": 7
        })))
        .unwrap();
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 7);
    }

    #[test]
    fn validates_mask_format_alpha_and_dimensions() {
        let mut source_bytes = Vec::new();
        let source = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
        source
            .write_to(
                &mut std::io::Cursor::new(&mut source_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let references = vec![
            ReferenceImage {
                name: "source.png".to_string(),
                media_type: "image/png".to_string(),
                bytes: source_bytes.clone(),
                is_mask: false,
                dimensions: (2, 2),
            },
            ReferenceImage {
                name: "mask.png".to_string(),
                media_type: "image/png".to_string(),
                bytes: source_bytes,
                is_mask: true,
                dimensions: (2, 2),
            },
        ];
        assert!(validate_mask_image(&references).is_ok());

        let mut mismatched = references;
        mismatched[1].dimensions = (1, 1);
        assert!(validate_mask_image(&mismatched).is_err());
    }
}
