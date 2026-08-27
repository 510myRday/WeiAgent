//! Per-request API key selection for providers with a key pool.
//!
//! Cursor state (round-robin counter, failover sticky index) lives in a
//! process-wide map keyed by provider id: selection stays lock-free on the
//! request path except for one short mutex hop per request, and nothing is
//! persisted — after a restart every pool simply starts from its first key.
//!
//! When a conversation affinity key is provided, round-robin and random pools
//! pin the *first* attempt of each request to a stable key for that affinity
//! so upstream prompt caches stay warm. Retries still sweep through the rest
//! of the pool.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use uuid::Uuid;

use crate::{
    domain::{ApiKeyStrategy, ModelProvider},
    error::AppResult,
};

#[derive(Default)]
struct KeyCursor {
    round_robin: usize,
    sticky: usize,
}

fn cursors() -> &'static Mutex<HashMap<Uuid, KeyCursor>> {
    static CURSORS: OnceLock<Mutex<HashMap<Uuid, KeyCursor>>> = OnceLock::new();
    CURSORS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_cursor<T>(provider_id: Uuid, apply: impl FnOnce(&mut KeyCursor) -> T) -> Option<T> {
    let mut cursors = cursors().lock().ok()?;
    Some(apply(cursors.entry(provider_id).or_default()))
}

/// `Uuid::new_v4` draws 122 bits from the OS RNG; reusing it keeps random key
/// picks unpredictable without adding a dedicated rand dependency.
fn random_index(len: usize) -> usize {
    (Uuid::new_v4().as_u128() % len as u128) as usize
}

fn affinity_index(affinity: &str, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let value = if let Ok(id) = Uuid::parse_str(affinity.trim()) {
        id.as_u128()
    } else {
        // FNV-1a 64-bit, widened for modulo.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in affinity.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        u128::from(hash)
    };
    (value % len as u128) as usize
}

/// Hands out the keys for the attempts of one upstream request.
///
/// The first attempt picks per the provider's strategy; retry attempts sweep
/// forward from the last used key so every retry lands on a different key
/// until the pool wraps. `note_failure` advances the failover cursor so the
/// *next* request also starts past a key that just errored.
#[derive(Debug)]
pub struct ApiKeyCycle {
    provider_id: Uuid,
    keys: Vec<String>,
    strategy: ApiKeyStrategy,
    last_index: Option<usize>,
    /// Conversation / prompt-cache affinity. Same value => same first key for
    /// round-robin and random pools.
    affinity: Option<String>,
}

impl ApiKeyCycle {
    pub fn new(provider: &ModelProvider) -> AppResult<Self> {
        Self::from_parts(provider.id, &provider.api_keys, provider.key_strategy, None)
    }

    /// Pin the first key pick to a stable conversation / cache affinity.
    pub fn with_affinity(mut self, affinity: Option<&str>) -> Self {
        self.affinity = affinity
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        self
    }

    pub(crate) fn from_parts(
        provider_id: Uuid,
        api_keys: &[String],
        strategy: ApiKeyStrategy,
        affinity: Option<String>,
    ) -> AppResult<Self> {
        let keys: Vec<String> = api_keys
            .iter()
            .map(|key| key.trim())
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
            .collect();
        // An empty pool represents an anonymous provider. Keeping one empty
        // entry lets the existing retry path issue exactly one unauthenticated
        // request while preserving key rotation for configured pools.
        let keys = if keys.is_empty() {
            vec![String::new()]
        } else {
            keys
        };
        Ok(Self {
            provider_id,
            keys,
            strategy,
            last_index: None,
            affinity: affinity
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }

    pub fn has_alternatives(&self) -> bool {
        self.keys.len() > 1
    }

    pub fn next_key(&mut self) -> &str {
        let len = self.keys.len();
        let index = match self.last_index {
            // Retry within the same request: sweep to the following key.
            Some(last) => (last + 1) % len,
            None => match self.strategy {
                ApiKeyStrategy::Failover => {
                    self.with_cursor(|cursor| cursor.sticky).unwrap_or_default() % len
                }
                ApiKeyStrategy::RoundRobin => {
                    if let Some(affinity) = self.affinity.as_deref() {
                        affinity_index(affinity, len)
                    } else {
                        self.with_cursor(|cursor| {
                            let index = cursor.round_robin;
                            cursor.round_robin = cursor.round_robin.wrapping_add(1);
                            index
                        })
                        .unwrap_or_default()
                            % len
                    }
                }
                ApiKeyStrategy::Random => self
                    .affinity
                    .as_deref()
                    .map(|affinity| affinity_index(affinity, len))
                    .unwrap_or_else(|| random_index(len)),
            },
        };
        self.last_index = Some(index);
        &self.keys[index]
    }

    pub(crate) fn current_key(&self) -> &str {
        self.last_index
            .and_then(|index| self.keys.get(index))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// The attempt on the current key failed. Failover moves its sticky
    /// cursor to the following key so subsequent requests skip the bad one.
    pub fn note_failure(&mut self) {
        let Some(last) = self.last_index else {
            return;
        };
        if self.strategy == ApiKeyStrategy::Failover {
            let next = (last + 1) % self.keys.len();
            self.with_cursor(|cursor| cursor.sticky = next);
            tracing::debug!(
                provider_id = %self.provider_id,
                from_key = last,
                to_key = next,
                "provider API key failed; failover cursor advanced"
            );
        }
    }

    fn with_cursor<T>(&self, apply: impl FnOnce(&mut KeyCursor) -> T) -> Option<T> {
        with_cursor(self.provider_id, apply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn provider(strategy: ApiKeyStrategy, keys: &[&str]) -> ModelProvider {
        ModelProvider {
            id: Uuid::new_v4(),
            provider_type: crate::domain::ProviderType::OpenAiResponses,
            name: "test".into(),
            base_url: "http://localhost".into(),
            api_keys: keys.iter().map(ToString::to_string).collect(),
            api_key_labels: vec![],
            key_strategy: strategy,
            models: vec!["model".into()],
            model_configs: vec![],
            extra_body: serde_json::Value::Null,
            prompt_cache: Default::default(),
            cache_capabilities: None,
            proxy_url: None,
            image_generation_url: None,
            image_edit_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn empty_or_blank_keys_still_issue_one_anonymous_slot() {
        let mut cycle = ApiKeyCycle::new(&provider(ApiKeyStrategy::Failover, &["  ", ""])).unwrap();
        assert_eq!(cycle.next_key(), "");
    }

    #[test]
    fn round_robin_rotates_across_requests() {
        let provider = provider(ApiKeyStrategy::RoundRobin, &["k1", "k2", "k3"]);
        let picks: Vec<_> = (0..3)
            .map(|_| ApiKeyCycle::new(&provider).unwrap().next_key().to_string())
            .collect();
        assert_eq!(picks, vec!["k1", "k2", "k3"]);
    }

    #[test]
    fn failover_sticks_until_a_failure_then_advances() {
        let provider = provider(ApiKeyStrategy::Failover, &["k1", "k2", "k3"]);
        let mut first = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(first.next_key(), "k1");

        let mut second = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(second.next_key(), "k1", "success keeps the sticky key");

        first.note_failure();
        let mut third = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(third.next_key(), "k2", "failure advances the sticky key");
    }

    #[test]
    fn failover_retries_sweep_the_pool() {
        let provider = provider(ApiKeyStrategy::Failover, &["k1", "k2", "k3"]);
        let mut cycle = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(cycle.next_key(), "k1");
        assert_eq!(cycle.next_key(), "k2");
        assert_eq!(cycle.next_key(), "k3");
        assert_eq!(cycle.next_key(), "k1", "sweep wraps around the pool");
    }

    #[test]
    fn random_picks_stay_in_range_and_retries_still_sweep() {
        let provider = provider(ApiKeyStrategy::Random, &["k1", "k2"]);
        for _ in 0..20 {
            let mut cycle = ApiKeyCycle::new(&provider).unwrap();
            let first = cycle.next_key().to_string();
            assert!(first == "k1" || first == "k2");
            let second = cycle.next_key().to_string();
            assert_ne!(first, second);
        }
    }

    #[test]
    fn round_robin_and_random_stick_to_conversation_affinity() {
        let conversation = Uuid::new_v4().to_string();
        let other = Uuid::new_v4().to_string();
        let rr = provider(ApiKeyStrategy::RoundRobin, &["k1", "k2", "k3", "k4"]);
        let random = provider(ApiKeyStrategy::Random, &["k1", "k2", "k3", "k4"]);

        let first = ApiKeyCycle::new(&rr)
            .unwrap()
            .with_affinity(Some(&conversation))
            .next_key()
            .to_string();
        let again = ApiKeyCycle::new(&rr)
            .unwrap()
            .with_affinity(Some(&conversation))
            .next_key()
            .to_string();
        assert_eq!(first, again);

        let random_first = ApiKeyCycle::new(&random)
            .unwrap()
            .with_affinity(Some(&conversation))
            .next_key()
            .to_string();
        let random_again = ApiKeyCycle::new(&random)
            .unwrap()
            .with_affinity(Some(&conversation))
            .next_key()
            .to_string();
        assert_eq!(random_first, random_again);

        // Different affinity may land on a different key; either way retries
        // still advance within the same request.
        let mut other_cycle = ApiKeyCycle::new(&rr).unwrap().with_affinity(Some(&other));
        let other_first = other_cycle.next_key().to_string();
        let other_retry = other_cycle.next_key().to_string();
        assert_ne!(other_first, other_retry);
    }
}
