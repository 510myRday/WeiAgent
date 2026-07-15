use crate::{
    error::AppResult,
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
            for choice in &chunk.choices {
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

                if let Some(reason) = choice.finish_reason.as_deref() {
                    events.push(Ok(if reason == "tool_calls" {
                        StreamEvent::FinishedWithToolCalls
                    } else {
                        StreamEvent::Finished
                    }));
                }
            }
            events
        }
        Err(error) => vec![Err(error.into())],
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
}
