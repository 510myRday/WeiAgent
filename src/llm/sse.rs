use crate::{
    error::{AppError, AppResult},
    llm::types::{StreamChunk, StreamEvent},
};

#[cfg(test)]
pub fn parse_sse_chunk(chunk: &str) -> Vec<AppResult<StreamEvent>> {
    chunk.lines().flat_map(parse_sse_line).collect()
}

pub fn parse_sse_line(line: &str) -> Vec<AppResult<StreamEvent>> {
    let Some(data) = line.strip_prefix("data:") else {
        return Vec::new();
    };
    let data = data.strip_prefix(' ').unwrap_or(data);
    if data == "[DONE]" {
        return vec![Ok(StreamEvent::Finished)];
    }

    let parsed: Result<StreamChunk, _> = serde_json::from_str(data);
    match parsed {
        Ok(chunk) => {
            let mut events = Vec::new();
            if let Some(usage) = chunk.usage {
                events.push(Ok(StreamEvent::Usage(usage.normalized())));
            }
            for choice in &chunk.choices {
                if let Some(ref reasoning) = choice.delta.reasoning_content
                    && !reasoning.is_empty()
                {
                    events.push(Ok(StreamEvent::ReasoningDelta(reasoning.clone())));
                }
                // Text content delta
                if let Some(ref content) = choice.delta.content
                    && !content.is_empty()
                {
                    events.push(Ok(StreamEvent::TextDelta(content.clone())));
                }

                // Tool calls delta
                if let Some(ref tool_calls) = choice.delta.tool_calls {
                    for tc in tool_calls {
                        if let (Some(id), Some(name)) =
                            (&tc.id, tc.function.as_ref().and_then(|f| f.name.as_ref()))
                            && !id.is_empty()
                            && !name.is_empty()
                        {
                            events.push(Ok(StreamEvent::ToolCallStart {
                                index: tc.index,
                                id: id.clone(),
                                name: name.clone(),
                            }));
                        }
                        if let Some(args) =
                            tc.function.as_ref().and_then(|f| f.arguments.as_deref())
                            && !args.is_empty()
                        {
                            events.push(Ok(StreamEvent::ToolCallArgsDelta {
                                index: tc.index,
                                arguments: args.to_string(),
                            }));
                        }
                    }
                }

                // SenseNova / some gateways send finish_reason:"" on intermediate
                // chunks instead of null. Treat blank as "not finished yet".
                if let Some(reason) = meaningful_finish_reason(choice.finish_reason.as_deref()) {
                    events.push(openai_finish_event(reason));
                }
            }
            events
        }
        Err(error) => vec![Err(error.into())],
    }
}

/// OpenAI-compatible providers usually use `null` while a stream is still
/// open. A few gateways (e.g. SenseNova) emit an empty string instead.
pub fn meaningful_finish_reason(reason: Option<&str>) -> Option<&str> {
    reason.map(str::trim).filter(|value| !value.is_empty())
}

pub fn openai_finish_event(reason: &str) -> AppResult<StreamEvent> {
    match reason.trim() {
        "stop" => Ok(StreamEvent::Finished),
        "tool_calls" | "function_call" => Ok(StreamEvent::FinishedWithToolCalls),
        "length" => Err(AppError::Llm(
            "OpenAI response stopped because the token limit was reached".to_string(),
        )),
        "content_filter" => Err(AppError::Llm(
            "OpenAI response was stopped by content filtering".to_string(),
        )),
        other => Err(AppError::Llm(format!(
            "OpenAI response ended with unsupported finish_reason {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_deltas() {
        let parsed = parse_sse_chunk(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\
             data: [DONE]\n",
        );
        let events: Vec<_> = parsed.into_iter().map(Result::unwrap).collect();
        assert!(matches!(&events[0], StreamEvent::TextDelta(s) if s == "hel"));
        assert!(matches!(&events[1], StreamEvent::TextDelta(s) if s == "lo"));
        assert!(matches!(&events[2], StreamEvent::Finished));
    }

    #[test]
    fn parses_reasoning_deltas_separately_from_answer_text() {
        let parsed = parse_sse_chunk(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"plan\"}}]}\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\
             data: [DONE]\n",
        );
        let events: Vec<_> = parsed.into_iter().map(Result::unwrap).collect();
        assert!(matches!(&events[0], StreamEvent::ReasoningDelta(s) if s == "plan"));
        assert!(matches!(&events[1], StreamEvent::TextDelta(s) if s == "answer"));
        assert!(matches!(&events[2], StreamEvent::Finished));
    }

    #[test]
    fn accepts_sse_data_without_a_space() {
        let parsed = parse_sse_line("data:[DONE]");
        assert!(matches!(
            parsed.into_iter().next().unwrap().unwrap(),
            StreamEvent::Finished
        ));
    }

    #[test]
    fn parses_tool_call_deltas() {
        let parsed = parse_sse_chunk(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"execute_terminal\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\
             data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"command\\\": \\\"ls\\\"}\"}}]},\"finish_reason\":null}]}\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n",
        );
        let events: Vec<_> = parsed.into_iter().map(Result::unwrap).collect();
        assert!(
            matches!(&events[0], StreamEvent::ToolCallStart { index: 0, id, name } if id == "call_1" && name == "execute_terminal")
        );
        assert!(matches!(
            &events[1],
            StreamEvent::ToolCallArgsDelta { index: 0, .. }
        ));
        assert!(matches!(&events[2], StreamEvent::FinishedWithToolCalls));
    }

    #[test]
    fn ignores_empty_tool_call_id_and_name_on_argument_deltas() {
        // ModelScope / DeepSeek stream subsequent tool-call chunks with id:"" and
        // name:"" instead of omitting those fields. Those empty strings must not
        // emit another ToolCallStart that would wipe a valid start.
        let parsed = parse_sse_chunk(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"execute_terminal\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\
             data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"\",\"type\":\"function\",\"function\":{\"name\":\"\",\"arguments\":\"{\\\"command\\\": \\\"echo hi\\\"}\"}}]},\"finish_reason\":null}]}\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n",
        );
        let events: Vec<_> = parsed.into_iter().map(Result::unwrap).collect();
        assert_eq!(events.len(), 3);
        assert!(
            matches!(&events[0], StreamEvent::ToolCallStart { index: 0, id, name } if id == "call_1" && name == "execute_terminal")
        );
        assert!(matches!(
            &events[1],
            StreamEvent::ToolCallArgsDelta {
                index: 0,
                arguments
            } if arguments == "{\"command\": \"echo hi\"}"
        ));
        assert!(matches!(&events[2], StreamEvent::FinishedWithToolCalls));
        assert!(!events.iter().any(|event| matches!(
                event,
                StreamEvent::ToolCallStart { id, name, .. } if id.is_empty() || name.is_empty()
        )));
    }

    #[test]
    fn keeps_final_tool_arguments_when_finish_reason_shares_the_chunk() {
        let parsed = parse_sse_line(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
        );
        let events: Vec<_> = parsed.into_iter().map(Result::unwrap).collect();
        assert!(matches!(
            &events[0],
            StreamEvent::ToolCallArgsDelta { arguments, .. } if arguments == "}"
        ));
        assert!(matches!(&events[1], StreamEvent::FinishedWithToolCalls));
    }

    #[test]
    fn finish_reason_stop_is_a_completion_event_without_done_sentinel() {
        let parsed =
            parse_sse_line("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}");
        assert!(matches!(
            parsed.into_iter().next().unwrap().unwrap(),
            StreamEvent::Finished
        ));
    }

    #[test]
    fn empty_finish_reason_is_ignored_like_null() {
        // SenseNova emits finish_reason:"" on intermediate reasoning/content chunks.
        let parsed = parse_sse_chunk(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"plan\"},\"finish_reason\":\"\"}]}\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"\"}]}\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
        );
        let events: Vec<_> = parsed.into_iter().map(Result::unwrap).collect();
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], StreamEvent::ReasoningDelta(s) if s == "plan"));
        assert!(matches!(&events[1], StreamEvent::TextDelta(s) if s == "ok"));
        assert!(matches!(&events[2], StreamEvent::Finished));
        assert!(meaningful_finish_reason(Some("")).is_none());
        assert!(meaningful_finish_reason(Some("   ")).is_none());
        assert_eq!(meaningful_finish_reason(Some("stop")), Some("stop"));
    }

    #[test]
    fn abnormal_finish_reason_is_an_error() {
        let parsed =
            parse_sse_line("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}");
        assert!(parsed.into_iter().next().unwrap().is_err());
    }
}
