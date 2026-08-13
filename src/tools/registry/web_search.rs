use async_trait::async_trait;
use serde_json::Value;

use super::{AgentTool, ToolExecutionContext, tool_definition};
use crate::{
    domain::{AgentToolsConfig, ToolResult},
    llm::types::ToolDefinition,
    tools::web_search::{
        WebSearchArgs, describe_web_search_sources,
        evidence::{
            WebExtractArgs, execute_web_extract_with_status_updates, record_web_search_provenance,
        },
        execute_web_search_with_status_updates,
    },
};

pub(super) struct WebSearchTool;

#[async_trait]
impl AgentTool for WebSearchTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "web_search",
            "Use for browsing, current information, source links, precise attribution, or information likely to have changed. Search complementary sources in parallel and return deduplicated evidence candidates, per-source diagnostics, and separately labeled AI research syntheses. Use fast for simple lookups, balanced for current facts and comparisons, and deep for comprehensive research or conflicting claims. For research, use complementary query rewrites, multiple independent source groups, and materially different retries when a search is empty. AI research syntheses are hypotheses and query directions, not verified evidence or independent sources; use web_extract on important URLs before citing them.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 500,
                        "description": "Search query. Prefer concrete keywords or a short question."
                    },
                    "additional_queries": {
                        "type": "array",
                        "maxItems": 3,
                        "items": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "description": "Optional complementary query rewrites for parallel retrieval. Use for deep research, ambiguous wording, or likely vocabulary differences."
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["auto", "fast", "balanced", "deep", "custom"],
                        "description": "Routing depth. fast uses one source at a time with fallback, balanced uses up to three independent sources in parallel, deep uses all eligible independent source groups, and custom follows source_ids/source_roles."
                    },
                    "source_roles": {
                        "type": "array",
                        "uniqueItems": true,
                        "items": {
                            "type": "string",
                            "enum": [
                                "general_search",
                                "semantic_research",
                                "current_news",
                                "official_docs",
                                "technical_research",
                                "academic_research",
                                "fact_checking",
                                "market_research",
                                "product_research",
                                "multilingual_search",
                                "ai_research",
                                "social_search"
                            ]
                        },
                        "description": "Optional capability roles used to constrain server-side routing."
                    },
                    "source_ids": {
                        "type": "array",
                        "uniqueItems": true,
                        "items": { "type": "string", "minLength": 1, "maxLength": 80 },
                        "description": "Optional source IDs from web_search_sources. Prefer automatic routing unless a source characteristic materially fits the task."
                    },
                    "max_results_per_source": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum native results requested from each selected source."
                    },
                    "allowed_domains": {
                        "type": "array",
                        "maxItems": 20,
                        "uniqueItems": true,
                        "items": { "type": "string", "minLength": 1, "maxLength": 253 },
                        "description": "Only return results from these domains or their subdomains."
                    },
                    "blocked_domains": {
                        "type": "array",
                        "maxItems": 20,
                        "uniqueItems": true,
                        "items": { "type": "string", "minLength": 1, "maxLength": 253 },
                        "description": "Exclude results from these domains and their subdomains."
                    },
                    "recency_days": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 3650,
                        "description": "Prefer or require results published within this many days when supported by the source."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.web_search.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args: WebSearchArgs = match serde_json::from_value(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid web_search arguments: {error}"),
                };
            }
        };
        let execution =
            execute_web_search_with_status_updates(args, &context.tools_config.web_search).await;
        record_web_search_provenance(&context.web_search_provenance, &execution.result);
        if let Err(error) = context
            .store
            .update_web_search_key_statuses(&execution.status_updates)
            .await
        {
            tracing::warn!(%error, "failed to persist web search key statuses");
        }
        execution.result
    }
}

pub(super) struct WebSearchSourcesTool;

#[async_trait]
impl AgentTool for WebSearchSourcesTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "web_search_sources",
            "List the currently configured web search sources, their roles, strengths, limitations, cost/latency tiers, evidence type, independence group, capabilities, and health. Use this before custom source selection so searches can use appropriate independent source groups; source state is dynamic and is intentionally not embedded in the stable web_search schema.",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.web_search.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        if !arguments.as_object().is_some_and(serde_json::Map::is_empty) {
            return ToolResult::Error {
                message: "web_search_sources accepts no arguments".to_string(),
            };
        }
        let output = describe_web_search_sources(&context.tools_config.web_search);
        match serde_json::to_string_pretty(&output) {
            Ok(stdout) => ToolResult::Completed {
                stdout,
                stderr: String::new(),
                exit_code: 0,
                duration_ms: 0,
                runtime: Some("builtin_web_search_sources".to_string()),
                output_cursor: 0,
                structured_content: None,
                is_error: false,
            },
            Err(error) => ToolResult::Error {
                message: format!("failed to serialize web search sources: {error}"),
            },
        }
    }
}

pub(super) struct WebExtractTool;

#[async_trait]
impl AgentTool for WebExtractTool {
    fn definition(&self, _config: &AgentToolsConfig) -> ToolDefinition {
        tool_definition(
            "web_extract",
            "Open one to five public HTTP(S) URLs for source verification. Prefer the most relevant original or primary pages, especially for important or disputed claims. Verify important or disputed claims with independent evidence, then cite the original page with a Markdown link next to the supported claim. Uses local extraction first, then configured URL-content providers when local content is blocked, dynamic, or low quality. Stores direct evidence, returns focused excerpts, and labels Grok SSO fallback output as AI-derived.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "urls": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 5,
                        "uniqueItems": true,
                        "items": { "type": "string", "format": "uri", "minLength": 8, "maxLength": 4096 },
                        "description": "Original result URLs to open and verify. Prefer primary sources."
                    },
                    "query": {
                        "type": "string",
                        "maxLength": 500,
                        "description": "Optional research question or keywords used to rank relevant excerpts."
                    },
                    "max_chars_per_url": {
                        "type": "integer",
                        "minimum": 1000,
                        "maximum": 20000,
                        "description": "Maximum excerpt characters returned per URL. Full extracted text remains in the evidence store."
                    }
                },
                "required": ["urls"],
                "additionalProperties": false
            }),
        )
    }

    fn is_enabled(&self, config: &AgentToolsConfig) -> bool {
        config.web_search.enabled
    }

    async fn execute(&self, arguments: Value, context: ToolExecutionContext) -> ToolResult {
        let args: WebExtractArgs = match serde_json::from_value(arguments) {
            Ok(args) => args,
            Err(error) => {
                return ToolResult::Error {
                    message: format!("Invalid web_extract arguments: {error}"),
                };
            }
        };
        let execution = execute_web_extract_with_status_updates(
            args,
            &context.store.data_dir,
            &context.tools_config.evidence_storage,
            &context.tools_config.web_search,
            &context.web_search_provenance,
        )
        .await;
        if let Err(error) = context
            .store
            .update_web_search_key_statuses(&execution.status_updates)
            .await
        {
            tracing::warn!(%error, "failed to persist web extraction key statuses");
        }
        execution.result
    }
}
