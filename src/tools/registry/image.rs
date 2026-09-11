use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::Value;

use super::{AgentTool, ToolExecutionContext, tool_definition};
use crate::{
    domain::{AgentToolsConfig, ImageAttachment, ImageSource, ToolResult},
    llm::types::ToolDefinition,
};

pub(super) const READ_IMAGES_TOOL_NAME: &str = "read_images";
const MAX_IMAGES: usize = 3;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4_096;

pub(super) struct ReadImagesTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadImagesArgs {
    paths: Vec<String>,
}

#[async_trait]
impl AgentTool for ReadImagesTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            READ_IMAGES_TOOL_NAME,
            "Read up to three local image files and return them as visual context. Use absolute file paths. Only use this when inspecting image contents is necessary.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_IMAGES,
                        "items": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": MAX_PATH_BYTES
                        },
                        "description": "Absolute paths to one to three PNG, JPEG, WEBP, or GIF files."
                    }
                },
                "required": ["paths"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.terminal.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        if !context.supports_multimodal {
            return ToolResult::Blocked {
                reason: "The selected model is not configured for multimodal image input"
                    .to_string(),
                code: Some("MultimodalRequired".to_string()),
                details: None,
            };
        }
        let args = match serde_json::from_value::<ReadImagesArgs>(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid read_images arguments: {error}"),
                };
            }
        };
        if args.paths.is_empty() || args.paths.len() > MAX_IMAGES {
            return ToolResult::Error {
                message: format!("paths must contain between 1 and {MAX_IMAGES} items"),
            };
        }

        let mut attachments = Vec::with_capacity(args.paths.len());
        let mut total_bytes = 0_usize;
        for path in args.paths {
            let path = path.trim();
            if path.is_empty() || path.len() > MAX_PATH_BYTES {
                return ToolResult::Error {
                    message: format!("image path must be between 1 and {MAX_PATH_BYTES} bytes"),
                };
            }
            let path = std::path::Path::new(path);
            if !path.is_absolute() {
                return ToolResult::Error {
                    message: format!("image path must be absolute: {}", path.display()),
                };
            }
            let bytes = match tokio::fs::read(path).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return ToolResult::Error {
                        message: format!("could not read image {}: {error}", path.display()),
                    };
                }
            };
            if bytes.len() > MAX_IMAGE_BYTES {
                return ToolResult::Error {
                    message: format!("image {} exceeds {MAX_IMAGE_BYTES} bytes", path.display()),
                };
            }
            total_bytes = total_bytes.saturating_add(bytes.len());
            if total_bytes > MAX_TOTAL_IMAGE_BYTES {
                return ToolResult::Error {
                    message: format!("selected images exceed {MAX_TOTAL_IMAGE_BYTES} bytes total"),
                };
            }
            let Some(media_type) = crate::image_util::detect_image_media_type(&bytes) else {
                return ToolResult::Error {
                    message: format!("unsupported or invalid image format: {}", path.display()),
                };
            };
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_string();
            attachments.push(ImageAttachment {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                media_type: media_type.to_string(),
                is_mask: false,
                source: ImageSource::Base64 {
                    data: STANDARD.encode(bytes),
                },
            });
        }

        let image_values = attachments
            .iter()
            .filter_map(|attachment| serde_json::to_value(attachment).ok())
            .collect::<Vec<_>>();
        let count = image_values.len();
        ToolResult::Completed {
            stdout: format!("图片读取成功：已读取 {count} 张图片，并已作为视觉上下文附加。"),
            stderr: String::new(),
            exit_code: 0,
            duration_ms: 0,
            runtime: Some("builtin_read_images".to_string()),
            cwd: None,
            output_cursor: 0,
            structured_content: Some(serde_json::json!({"images": image_values})),
            is_error: false,
        }
    }
}
