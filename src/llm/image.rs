use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::multipart;
use serde_json::{Map, Value, json};

use crate::{
    domain::{ImageAttachment, ImageModelConfig, ImageSource, ModelProvider, ProviderType},
    error::{AppError, AppResult},
    llm::{
        client::RequestOptions,
        http::{DEFAULT_MODEL_LIST_TIMEOUT, LlmHttp, read_response_body_limited},
        keys::ApiKeyCycle,
    },
};

#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
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
    async fn generate(
        &self,
        provider: &ModelProvider,
        model: &str,
        prompt: &str,
        references: &[ImageAttachment],
        options: RequestOptions,
        image_options: ImageRequestOptions,
        log_context: Option<ImageLogContext>,
    ) -> AppResult<Vec<GeneratedImage>>;

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
        client: &reqwest::Client,
        response: reqwest::Response,
        options: &RequestOptions,
        log_context: Option<&ImageLogContext>,
        request_json: &str,
        endpoint: &str,
        operation: &str,
        started: Instant,
    ) -> AppResult<Vec<GeneratedImage>> {
        let status = response.status();
        let mut body_for_log = None;
        let result = async {
            let body = read_response_body_limited(response, options.timeout).await?;
            body_for_log = Some(body.clone());
            if !status.is_success() {
                if status == reqwest::StatusCode::NOT_FOUND {
                    let hint = if operation == "image edit" {
                        "The selected provider may expose image generation but not image editing; configure its image edit URL or choose a provider/model with image editing support"
                    } else {
                        "Check the provider Base URL or configure its image generation URL"
                    };
                    return Err(AppError::Llm(format!(
                        "{operation} endpoint returned 404: {endpoint}. {hint}. Upstream response: {body}"
                    )));
                }
                return Err(AppError::Llm(format!(
                    "image upstream returned {status} from {endpoint}: {body}"
                )));
            }
            let value: Value = serde_json::from_str(&body)
                .map_err(|error| AppError::Llm(format!("invalid image upstream JSON: {error}")))?;
            let items = value["data"].as_array().ok_or_else(|| {
                AppError::Llm("image upstream response is missing data".to_string())
            })?;
            let mut images = Vec::with_capacity(items.len());
            for item in items {
                if let Some(encoded) = item["b64_json"].as_str() {
                    let bytes = STANDARD.decode(encoded).map_err(|error| {
                        AppError::Llm(format!("invalid generated image Base64: {error}"))
                    })?;
                    let media_type = detected_media_type(&bytes, "image/png");
                    images.push(GeneratedImage { bytes, media_type });
                } else if let Some(url) = item["url"].as_str() {
                    let image = tokio::time::timeout(options.timeout, client.get(url).send())
                        .await
                        .map_err(|_| {
                            AppError::Llm("generated image URL request timed out".to_string())
                        })?
                        .map_err(|error| {
                            AppError::Llm(format!("failed to download generated image: {error}"))
                        })?;
                    let media_type = image
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .filter(|value| value.starts_with("image/"))
                        .unwrap_or("image/png")
                        .split(';')
                        .next()
                        .unwrap_or("image/png")
                        .trim()
                        .to_string();
                    let bytes = image.bytes().await.map_err(|error| {
                        AppError::Llm(format!("failed to read generated image: {error}"))
                    })?;
                    let media_type = detected_media_type(&bytes, &media_type);
                    images.push(GeneratedImage {
                        bytes: bytes.to_vec(),
                        media_type,
                    });
                }
            }
            if images.is_empty() {
                return Err(AppError::Llm(
                    "image upstream returned no image data".to_string(),
                ));
            }
            Ok(images)
        }
        .await;
        let error_message = result.as_ref().err().map(ToString::to_string);
        log_image_request(
            &self.http,
            log_context,
            request_json,
            body_for_log.as_deref(),
            Some(status.as_u16()),
            error_message.as_deref(),
            started,
        );
        result
    }

    async fn reference_bytes(
        &self,
        client: &reqwest::Client,
        attachment: &ImageAttachment,
        timeout: std::time::Duration,
    ) -> AppResult<Vec<u8>> {
        match &attachment.source {
            ImageSource::Base64 { data } => STANDARD
                .decode(data)
                .map_err(|error| AppError::BadRequest(format!("invalid reference image: {error}"))),
            ImageSource::Url { url } => {
                let response = tokio::time::timeout(timeout, client.get(url).send())
                    .await
                    .map_err(|_| AppError::Llm("reference image request timed out".to_string()))?
                    .map_err(|error| {
                        AppError::Llm(format!("failed to fetch reference image: {error}"))
                    })?;
                let status = response.status();
                if !status.is_success() {
                    return Err(AppError::Llm(format!("reference image returned {status}")));
                }
                Ok(response
                    .bytes()
                    .await
                    .map_err(|error| {
                        AppError::Llm(format!("failed to read reference image: {error}"))
                    })?
                    .to_vec())
            }
            ImageSource::Local { .. } => Err(AppError::BadRequest(
                "local generated images cannot be sent as new references".to_string(),
            )),
        }
    }
}

#[async_trait]
impl ImageClient for OpenAiImagesClient {
    async fn generate(
        &self,
        provider: &ModelProvider,
        model: &str,
        prompt: &str,
        references: &[ImageAttachment],
        options: RequestOptions,
        image_options: ImageRequestOptions,
        log_context: Option<ImageLogContext>,
    ) -> AppResult<Vec<GeneratedImage>> {
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
        let mut keys = ApiKeyCycle::new(provider)?;
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
            let url = Self::generation_url(provider);
            let request_json = serde_json::to_string_pretty(&body).unwrap_or_default();
            let started = Instant::now();
            let response = match self
                .http
                .retry_send(&options, &mut keys, |key| {
                    client.post(&url).bearer_auth(key).json(&body)
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
                    &client,
                    response,
                    &options,
                    log_context.as_ref(),
                    &request_json,
                    &url,
                    "image generation",
                    started,
                )
                .await;
        }

        let mut references_bytes = Vec::with_capacity(references.len());
        for reference in references {
            references_bytes.push((
                reference.name.clone(),
                reference.media_type.clone(),
                self.reference_bytes(&client, reference, options.timeout)
                    .await?,
            ));
        }
        let url = Self::edit_url(provider);
        let prompt = prompt.to_string();
        let model = model.to_string();
        let request_json = serde_json::to_string_pretty(&json!({
            "model": model,
            "prompt": prompt,
            "references": references.iter().map(|reference| json!({
                "name": reference.name,
                "media_type": reference.media_type,
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
                for (name, media_type, bytes) in &references_bytes {
                    let part = multipart::Part::bytes(bytes.clone())
                        .file_name(name.clone())
                        .mime_str(media_type)
                        .unwrap_or_else(|_| {
                            multipart::Part::bytes(bytes.clone()).file_name(name.clone())
                        });
                    form = form.part("image[]", part);
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
                client.post(&url).bearer_auth(key).multipart(form)
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
            &client,
            response,
            &options,
            log_context.as_ref(),
            &request_json,
            &url,
            "image edit",
            started,
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
        };
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let response = self
            .http
            .retry_send(&options, &mut keys, |key| client.get(&url).bearer_auth(key))
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
        http.log_enabled(),
        &context.agent_name,
        context.agent_id.as_deref(),
        context.conversation_id.as_deref(),
        &context.model_id,
        &context.provider_name,
        request_json,
        response,
        status,
        error,
        crate::storage::llm_logs::LlmLogMetrics::timed(started),
    );
}

fn detected_media_type(bytes: &[u8], fallback: &str) -> String {
    match ::image::guess_format(bytes).ok() {
        Some(::image::ImageFormat::Png) => "image/png".to_string(),
        Some(::image::ImageFormat::Jpeg) => "image/jpeg".to_string(),
        Some(::image::ImageFormat::WebP) => "image/webp".to_string(),
        Some(::image::ImageFormat::Gif) => "image/gif".to_string(),
        _ => fallback.to_string(),
    }
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
    use super::default_image_endpoint;

    #[test]
    fn default_image_endpoint_trims_trailing_slashes() {
        assert_eq!(
            default_image_endpoint("https://api.example.test/v1///", "edits"),
            "https://api.example.test/v1/images/edits"
        );
    }
}
