use crate::{error::AppResult, llm::types::StreamChunk};

#[cfg(test)]
pub fn parse_sse_chunk(chunk: &str) -> Vec<AppResult<String>> {
    chunk.lines().flat_map(parse_sse_line).collect()
}

pub fn parse_sse_line(line: &str) -> Vec<AppResult<String>> {
    let Some(data) = line.strip_prefix("data: ") else {
        return Vec::new();
    };
    if data == "[DONE]" {
        return Vec::new();
    }

    let parsed = serde_json::from_str::<StreamChunk>(data).map(|chunk| {
        chunk
            .choices
            .into_iter()
            .filter_map(|choice| choice.delta.content)
            .collect::<String>()
    });

    match parsed {
        Ok(content) if content.is_empty() => Vec::new(),
        Ok(content) => vec![Ok(content)],
        Err(error) => vec![Err(error.into())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_compatible_sse_chunks() {
        let parsed = parse_sse_chunk(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\
             data: [DONE]\n",
        );

        let values = parsed.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        assert_eq!(values, vec!["hel", "lo"]);
    }
}
