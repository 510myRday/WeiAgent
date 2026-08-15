use async_trait::async_trait;
use serde_json::Value;

use super::{
    AgentTool, ToolExecutionContext, frozen_terminal_config, normalize_optional_cwd,
    run_with_approval, tool_definition,
};
use crate::{
    domain::{AgentToolsConfig, ToolResult},
    llm::types::ToolDefinition,
    tools::{
        multi_edit::{
            MULTI_EDIT_GRANT_COMMAND, MULTI_EDIT_TOOL_NAME, MultiEditFileArgs, MultiEditRequest,
            execute_multi_edit, parse_multi_edit_file_args,
        },
        terminal::SandboxPermissions,
    },
};

pub(super) struct MultiEditFileTool;

#[async_trait]
impl AgentTool for MultiEditFileTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        let description = "Edit, create, delete, or move text files in one atomic call. Prefer this over shell redirection or execute_terminal file rewrites for source, config, and documentation changes. Every operation is validated before anything is written; on failure files are left unchanged. Result warnings (for example possibly unbalanced delimiters) do not block the write; inspect them before continuing. For an edit, read the current file first when it is not already in context, keep each old_string short but unique, and put all replacements for one path in one edit operation. To fully rewrite an existing file, use a single create operation with overwrite=true instead of many edits. `operations` must be a JSON array, and each object inside it must include its own `type` field: `edit`, `create`, `delete`, or `move`; `type` is not a top-level field. Order edits so a later old_string never targets text produced by an earlier new_string; such edits are rejected. In TS/JS template strings write ${{...}} exactly; do not escape the dollar sign as \\${{...}}. In-workspace edits may require approval under request_approval. When cwd or a target path is outside the configured workspace roots, the server requests approval automatically before writing; sandbox_permissions=require_escalated with a short justification may also be supplied to make the expanded access request explicit. Do not bypass multi_edit_file via shell when an outside-root edit is needed.";
        tool_definition(
            MULTI_EDIT_TOOL_NAME,
            &description,
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "operations": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 32,
                        "description": "Preferred ordered file operations. A path may appear only once per call. Pass a JSON array, not a quoted JSON string. Each array object must include its own type field; type belongs inside operations[], not at the top level.",
                        "items": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "description": "Apply exact text replacements inside one existing file. Include type=edit in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "path", "edits"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["edit"]},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "File to edit, relative to cwd unless absolute."},
                                        "edits": {
                                            "type": "array",
                                            "minItems": 1,
                                            "maxItems": 64,
                                            "description": "Ordered replacements for this file, validated and applied in memory before any write.",
                                            "items": {
                                                "type": "object",
                                                "additionalProperties": false,
                                                "required": ["old_string", "new_string"],
                                                "properties": {
                                                    "old_string": {"type": "string", "maxLength": 200000, "description": "Exact text copied from the current file, including indentation, without line-number prefixes. Compared after normalizing CRLF to LF."},
                                                    "new_string": {"type": "string", "maxLength": 2097152, "description": "Replacement text. Must differ from old_string."},
                                                    "replace_all": {"type": "boolean", "description": "Replace every occurrence of old_string. Defaults to false."},
                                                    "start_line": {"type": "integer", "minimum": 1, "description": "Optional 1-based line hint used only when old_string matches more than once; the occurrence closest to this line is replaced."}
                                                }
                                            }
                                        }
                                    }
                                },
                                {
                                    "type": "object",
                                    "description": "Create a new file, or fully rewrite an existing one with overwrite=true. Include type=create in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "path", "content"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["create"]},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "File to create. Parent directories are created automatically."},
                                        "content": {"type": "string", "maxLength": 2097152, "description": "Full file content to write."},
                                        "overwrite": {"type": "boolean", "description": "Defaults to false, which fails when the target already exists. Set true to replace the existing file; this is the preferred way to fully rewrite a file."}
                                    }
                                },
                                {
                                    "type": "object",
                                    "description": "Delete one file. Directories are refused. Include type=delete in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "path"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["delete"]},
                                        "path": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "File to delete."}
                                    }
                                },
                                {
                                    "type": "object",
                                    "description": "Move or rename one file. Directories are refused. Include type=move in this object.",
                                    "additionalProperties": false,
                                    "required": ["type", "from", "to"],
                                    "properties": {
                                        "type": {"type": "string", "enum": ["move"]},
                                        "from": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "Source file."},
                                        "to": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "Target path. Parent directories are created automatically."},
                                        "overwrite": {"type": "boolean", "description": "Defaults to false, which fails when the target already exists. Set true to replace the target file."}
                                    }
                                }
                            ]
                        }
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Existing working directory for relative paths; a ~ prefix expands to the user home directory. Omit to use the default workspace. If cwd or target files are outside the configured workspace roots, the server automatically asks the user before writing."
                    },
                    "sandbox_permissions": {
                        "type": "string",
                        "enum": ["use_default", "require_escalated"],
                        "description": "Write privilege request. use_default follows the server policy and automatically requests approval for outside-root cwd or target paths. Use require_escalated with a short justification when expanded access should be explicit. Prefer this over rewriting files through execute_terminal."
                    },
                    "justification": {
                        "type": "string",
                        "maxLength": 2000,
                        "description": "Short reason for elevated write access. Include it when sandbox_permissions is require_escalated. Max 2000 bytes. Skip it for use_default."
                    }
                },
                "required": ["operations"]
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.terminal.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args: MultiEditFileArgs = match parse_multi_edit_file_args(arguments) {
            Ok(args) => args,
            Err(message) => return ToolResult::Error { message },
        };
        let cwd = match normalize_optional_cwd(args.cwd.clone()) {
            Ok(cwd) => cwd,
            Err(message) => return ToolResult::Error { message },
        };
        let terminal_config = frozen_terminal_config(&context);
        if terminal_config.permission_mode == crate::domain::TerminalPermissionMode::RequestApproval
            && args.sandbox_permissions == SandboxPermissions::RequireEscalated
        {
            let justification = args
                .justification
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if justification.is_none() || justification.is_some_and(|value| value.len() > 2000) {
                return ToolResult::Error {
                    message: "justification is required for require_escalated and must be at most 2000 bytes"
                        .to_string(),
                };
            }
        }
        let justification = args.justification.clone();
        let args = MultiEditFileArgs {
            cwd: cwd.clone(),
            ..args
        };

        let result = execute_multi_edit(
            &MultiEditRequest {
                args: args.clone(),
                approved: false,
            },
            &terminal_config,
            &context.store.terminal_workspace_dir,
        )
        .await;

        let ToolResult::PendingApproval {
            approval_id,
            command,
        } = &result
        else {
            return result;
        };
        let approval_id = approval_id.clone();
        let command = command.clone();
        let approval_cwd = cwd.clone().or_else(|| {
            Some(crate::fsutil::display_path(
                &context.store.terminal_workspace_dir,
            ))
        });
        let justification = justification
            .filter(|value| !value.trim().is_empty())
            .or_else(|| Some("Apply multi_edit_file changes".to_string()));
        run_with_approval(
            &context,
            approval_id,
            command,
            approval_cwd.clone(),
            "builtin_multi_edit",
            "file_write",
            justification,
            false,
            "This edit requires interactive approval; use the SSE message endpoint",
            "Edit rejected by user or approval expired",
            MULTI_EDIT_GRANT_COMMAND,
            approval_cwd.as_deref(),
            async |terminal_config| {
                execute_multi_edit(
                    &MultiEditRequest {
                        args,
                        approved: true,
                    },
                    &terminal_config,
                    &context.store.terminal_workspace_dir,
                )
                .await
            },
        )
        .await
    }
}
