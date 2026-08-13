use serde_json::Value;

use crate::domain::ToolResult;

const MAX_TOOL_RESULT_STDOUT_CHARS: usize = 24_000;
const MAX_TOOL_RESULT_STDERR_CHARS: usize = 4_000;
const MAX_WEB_SEARCH_SNIPPET_CHARS: usize = 600;
const MAX_WEB_SEARCH_CONTENT_EXCERPT_CHARS: usize = 2_000;
const MAX_WEB_SEARCH_RESULTS_FOR_LLM: usize = 15;
const MAX_WEB_EXTRACT_EXCERPT_CHARS: usize = 8_000;

pub(super) fn sanitize_tool_result_for_llm(result: &ToolResult) -> Value {
    match result {
        ToolResult::Completed {
            stdout,
            stderr,
            exit_code,
            runtime,
            structured_content,
            is_error,
            ..
        } => {
            let mut map = serde_json::Map::new();
            map.insert("status".to_string(), Value::String("completed".to_string()));
            map.insert(
                "stdout".to_string(),
                Value::String(compact_tool_stdout(stdout)),
            );
            let stderr = trim_chars(stderr, MAX_TOOL_RESULT_STDERR_CHARS);
            if !stderr.is_empty() {
                map.insert("stderr".to_string(), Value::String(stderr));
            }
            map.insert("exit_code".to_string(), Value::from(*exit_code as i64));
            if let Some(runtime) = runtime {
                map.insert("runtime".to_string(), Value::String(runtime.clone()));
            }
            if let Some(content) = structured_content {
                map.insert("structured_content".to_string(), content.clone());
            }
            if *is_error {
                map.insert("is_error".to_string(), Value::Bool(true));
            }
            Value::Object(map)
        }
        ToolResult::Running {
            process_id,
            message,
            stdout,
            stderr,
            runtime,
            output_cursor,
            ..
        } => {
            let mut map = serde_json::Map::new();
            map.insert("status".to_string(), Value::String("running".to_string()));
            map.insert("process_id".to_string(), Value::String(process_id.clone()));
            map.insert("message".to_string(), Value::String(message.clone()));
            let stdout = compact_tool_stdout(stdout);
            if !stdout.is_empty() {
                map.insert("stdout".to_string(), Value::String(stdout));
            }
            let stderr = trim_chars(stderr, MAX_TOOL_RESULT_STDERR_CHARS);
            if !stderr.is_empty() {
                map.insert("stderr".to_string(), Value::String(stderr));
            }
            if let Some(runtime) = runtime {
                map.insert("runtime".to_string(), Value::String(runtime.clone()));
            }
            if *output_cursor > 0 {
                map.insert("output_cursor".to_string(), Value::from(*output_cursor));
            }
            Value::Object(map)
        }
        ToolResult::PendingApproval { command, .. } => serde_json::json!({
            "status": "pending_approval",
            "command": command,
        }),
        ToolResult::Blocked {
            reason,
            code,
            details,
        } => {
            let mut map = serde_json::Map::new();
            map.insert("status".to_string(), Value::String("blocked".to_string()));
            map.insert("reason".to_string(), Value::String(reason.clone()));
            if let Some(code) = code {
                map.insert("code".to_string(), Value::String(code.clone()));
            }
            if let Some(details) = details {
                map.insert("details".to_string(), details.clone());
            }
            Value::Object(map)
        }
        ToolResult::Rejected { reason } => serde_json::json!({
            "status": "rejected",
            "reason": reason,
        }),
        ToolResult::Interrupted { reason } => serde_json::json!({
            "status": "interrupted",
            "reason": reason,
        }),
        ToolResult::Error { message } => serde_json::json!({
            "status": "error",
            "message": message,
        }),
    }
}

fn compact_tool_stdout(stdout: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(stdout) {
        if compact_web_search_value(&mut value) {
            return value.to_string();
        }
        if compact_web_extract_value(&mut value) {
            return trim_chars(&value.to_string(), 48_000);
        }
    }
    trim_chars(stdout, MAX_TOOL_RESULT_STDOUT_CHARS)
}

fn compact_web_search_value(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let is_search = obj.get("queries").is_some()
        && obj.get("source_runs").is_some()
        && obj.get("results").is_some();
    if !is_search {
        return false;
    }
    if let Some(results) = obj.get_mut("results").and_then(Value::as_array_mut) {
        if results.len() > MAX_WEB_SEARCH_RESULTS_FOR_LLM {
            results.truncate(MAX_WEB_SEARCH_RESULTS_FOR_LLM);
        }
        for hit in results.iter_mut() {
            if let Some(hit_obj) = hit.as_object_mut() {
                hit_obj.remove("score");
                if let Some(snippet) = hit_obj.get("snippet").and_then(Value::as_str) {
                    let trimmed = trim_chars(snippet, MAX_WEB_SEARCH_SNIPPET_CHARS);
                    hit_obj.insert("snippet".to_string(), Value::String(trimmed));
                }
                if let Some(content) = hit_obj.get("content_excerpt").and_then(Value::as_str) {
                    let trimmed = trim_chars(content, MAX_WEB_SEARCH_CONTENT_EXCERPT_CHARS);
                    hit_obj.insert("content_excerpt".to_string(), Value::String(trimmed));
                }
            }
        }
    }
    if let Some(syntheses) = obj.get_mut("ai_syntheses").and_then(Value::as_array_mut) {
        syntheses.truncate(6);
        for synthesis in syntheses {
            if let Some(answer) = synthesis.get("answer").and_then(Value::as_str) {
                let answer = trim_chars(answer, 1_200);
                if let Some(map) = synthesis.as_object_mut() {
                    map.insert("answer".to_string(), Value::String(answer));
                }
            }
        }
    }
    if let Some(results) = obj.get("results").and_then(Value::as_array) {
        let count = results.len() as u64;
        obj.insert("result_count".to_string(), Value::from(count));
    }
    true
}

fn compact_web_extract_value(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    if obj.get("citation_instruction").is_none() {
        return false;
    }
    let Some(documents) = obj.get_mut("documents").and_then(Value::as_array_mut) else {
        return false;
    };
    for document in documents {
        let Some(excerpts) = document
            .get_mut("relevant_excerpts")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let mut remaining = MAX_WEB_EXTRACT_EXCERPT_CHARS;
        for excerpt in excerpts.iter_mut() {
            if remaining == 0 {
                *excerpt = Value::String(String::new());
                continue;
            }
            let Some(text) = excerpt.as_str() else {
                continue;
            };
            let trimmed = trim_chars(text, remaining.min(2_000));
            remaining = remaining.saturating_sub(trimmed.chars().count());
            *excerpt = Value::String(trimmed);
        }
        excerpts.retain(|excerpt| excerpt.as_str().is_some_and(|text| !text.is_empty()));
    }
    true
}

pub(super) fn trim_chars(input: &str, max_chars: usize) -> String {
    crate::textutil::truncate_with_omission_notice(input, max_chars)
}
