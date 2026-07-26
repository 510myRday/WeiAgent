//! Per-request API key selection for providers with a key pool.
//!
//! Cursor state (round-robin counter, failover sticky index) lives in a
//! process-wide map keyed by provider id: selection stays lock-free on the
//! request path except for one short mutex hop per request, and nothing is
//! persisted — after a restart every pool simply starts from its first key.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use uuid::Uuid;

use crate::{
    domain::{ApiKeyStrategy, ModelProvider},
    error::{AppError, AppResult},
};

/// Trying every key of a huge pool inside one request would multiply the
/// worst-case latency; past this many attempts the remaining keys are only
/// reached by later requests via the advancing failover cursor.
const MAX_KEY_ATTEMPTS_PER_REQUEST: u32 = 10;

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
}

impl ApiKeyCycle {
    pub fn new(provider: &ModelProvider) -> AppResult<Self> {
        Self::from_parts(provider.id, &provider.api_keys, provider.key_strategy)
    }

    pub(crate) fn from_parts(
        provider_id: Uuid,
        api_keys: &[String],
        strategy: ApiKeyStrategy,
    ) -> AppResult<Self> {
        let keys: Vec<String> = api_keys
            .iter()
            .map(|key| key.trim())
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
            .collect();
        if keys.is_empty() {
            return Err(AppError::MissingApiKey);
        }
        Ok(Self {
            provider_id,
            keys,
            strategy,
            last_index: None,
        })
    }

    pub fn has_alternatives(&self) -> bool {
        self.keys.len() > 1
    }

    /// How many attempts one request may spend on this pool, at minimum.
    pub fn attempt_budget(&self) -> u32 {
        (self.keys.len() as u32).min(MAX_KEY_ATTEMPTS_PER_REQUEST)
    }

    pub fn next_key(&mut self) -> &str {
        let len = self.keys.len();
        let index = match self.last_index {
            // Retry within the same request: sweep to the following key.
            Some(last) => (last + 1) % len,
            None => match self.strategy {
                ApiKeyStrategy::Failover => self
                    .with_cursor(|cursor| cursor.sticky)
                    .unwrap_or_default()
                    % len,
                ApiKeyStrategy::RoundRobin => self
                    .with_cursor(|cursor| {
                        let index = cursor.round_robin;
                        cursor.round_robin = cursor.round_robin.wrapping_add(1);
                        index
                    })
                    .unwrap_or_default()
                    % len,
                ApiKeyStrategy::Random => random_index(len),
            },
        };
        self.last_index = Some(index);
        &self.keys[index]
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
    use chrono::Utc;

    use super::*;

    fn provider(strategy: ApiKeyStrategy, keys: &[&str]) -> ModelProvider {
        ModelProvider {
            id: Uuid::new_v4(),
            provider_type: Default::default(),
            name: "test".to_string(),
            base_url: "http://localhost".to_string(),
            api_keys: keys.iter().map(ToString::to_string).collect(),
            key_strategy: strategy,
            models: vec!["model".to_string()],
            model_configs: Vec::new(),
            extra_body: serde_json::Value::Null,
            prompt_cache: Default::default(),
            cache_capabilities: None,
            proxy_url: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn empty_pool_is_a_missing_key_error() {
        let error = ApiKeyCycle::new(&provider(ApiKeyStrategy::Failover, &["  ", ""])).unwrap_err();
        assert!(matches!(error, AppError::MissingApiKey));
    }

    #[test]
    fn round_robin_rotates_across_requests() {
        let provider = provider(ApiKeyStrategy::RoundRobin, &["k1", "k2", "k3"]);
        let picks: Vec<String> = (0..4)
            .map(|_| {
                ApiKeyCycle::new(&provider)
                    .unwrap()
                    .next_key()
                    .to_string()
            })
            .collect();
        assert_eq!(picks, ["k1", "k2", "k3", "k1"]);
    }

    #[test]
    fn failover_sticks_until_a_failure_then_advances() {
        let provider = provider(ApiKeyStrategy::Failover, &["k1", "k2", "k3"]);

        let mut first = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(first.next_key(), "k1");
        drop(first);
        let mut second = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(second.next_key(), "k1", "success keeps the sticky key");
        second.note_failure();

        let mut third = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(third.next_key(), "k2", "failure advances the sticky key");
    }

    #[test]
    fn retries_sweep_forward_within_one_request() {
        let provider = provider(ApiKeyStrategy::Failover, &["k1", "k2", "k3"]);
        let mut cycle = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(cycle.next_key(), "k1");
        cycle.note_failure();
        assert_eq!(cycle.next_key(), "k2");
        cycle.note_failure();
        assert_eq!(cycle.next_key(), "k3");
        cycle.note_failure();
        assert_eq!(cycle.next_key(), "k1", "sweep wraps around the pool");
    }

    #[test]
    fn random_picks_stay_in_range_and_retries_still_sweep() {
        let provider = provider(ApiKeyStrategy::Random, &["k1", "k2"]);
        for _ in 0..16 {
            let mut cycle = ApiKeyCycle::new(&provider).unwrap();
            let first = cycle.next_key().to_string();
            assert!(["k1", "k2"].contains(&first.as_str()));
            let second = cycle.next_key().to_string();
            assert_ne!(first, second, "the retry moves to the other key");
        }
    }

    #[test]
    fn attempt_budget_is_capped() {
        let keys: Vec<String> = (0..24).map(|index| format!("k{index}")).collect();
        let mut provider = provider(ApiKeyStrategy::Failover, &[]);
        provider.api_keys = keys;
        let cycle = ApiKeyCycle::new(&provider).unwrap();
        assert_eq!(cycle.attempt_budget(), 10);
    }
}
