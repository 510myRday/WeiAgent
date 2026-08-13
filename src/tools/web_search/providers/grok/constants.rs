// Grok provider internals.
use std::time::Duration;

use base64::Engine as _;

pub(super) const DEFAULT_MODE: &str = "fast";
pub(super) const CONNECTIVITY_PROBE_PROMPT: &str = "Reply with exactly: PON";
pub(super) const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";
pub(super) const X0_FALLBACK_STATSIG_ID: &str = "eDA6VHlwZUVycm9yOiBDYW5ub3QgcmVhZCBwcm9wZXJ0aWVzIG9mIHVuZGVmaW5lZCAocmVhZGluZyBjaGlsZE5vZGVzKQ==";
pub(super) const MAX_FRAME_BYTES: usize = 8 << 20;
pub(super) const MAX_SNIPPET_CHARS: usize = 800;
pub(super) const STATSIG_CACHE_TTL: Duration = Duration::from_secs(50 * 60);

pub(super) fn valid_statsig_id(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value));
    matches!(decoded, Ok(bytes) if bytes.len() == 70)
}

/// Frontend failure fallback used when module 4629918 signing fails: btoa("x0:" + error).
pub(super) fn is_x0_fallback_statsig_id(value: &str) -> bool {
    value.trim() == X0_FALLBACK_STATSIG_ID
}

pub(super) fn usable_statsig_id(value: &str) -> bool {
    valid_statsig_id(value) || is_x0_fallback_statsig_id(value)
}

