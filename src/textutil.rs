/// Shared string truncation helpers used by LLM sanitization, compaction, and
/// provider error messages.
pub fn truncate_chars(value: &str, max: usize) -> String {
    truncate_with(value, max, "…")
}

pub fn truncate_with(value: &str, max: usize, ellipsis: &str) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}{ellipsis}")
    } else {
        head
    }
}

/// Keep the head of `input` and append an explicit omitted-char notice.
pub fn truncate_with_omission_notice(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    let keep = max_chars.saturating_sub(32);
    let mut out: String = input.chars().take(keep).collect();
    out.push_str(&format!(
        "\n...[truncated {omitted} chars]",
        omitted = count - keep
    ));
    out
}

/// Keep head and tail so a compacted summary still has start and end context.
pub fn truncate_preserving_tail(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    if max_chars < 64 {
        return input.chars().take(max_chars).collect();
    }
    let keep_head = max_chars.saturating_mul(70) / 100;
    let keep_tail = max_chars.saturating_sub(keep_head + 32);
    let head: String = input.chars().take(keep_head).collect();
    let tail: String = input
        .chars()
        .rev()
        .take(keep_tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!(
        "{head}\n…[truncated {omitted} chars]…\n{tail}",
        omitted = count - keep_head - keep_tail
    )
}
