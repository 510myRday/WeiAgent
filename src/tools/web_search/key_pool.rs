use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::domain::WebSearchKeySelection;

static ROUND_ROBIN_COUNTERS: OnceLock<dashmap::DashMap<String, AtomicUsize>> = OnceLock::new();

fn counters() -> &'static dashmap::DashMap<String, AtomicUsize> {
    ROUND_ROBIN_COUNTERS.get_or_init(dashmap::DashMap::new)
}

/// Produce a key attempt order for one source.
pub fn key_attempt_order(
    source_id: &str,
    key_count: usize,
    selection: &WebSearchKeySelection,
) -> Vec<usize> {
    if key_count == 0 {
        return Vec::new();
    }
    match selection {
        WebSearchKeySelection::Sequential => (0..key_count).collect(),
        WebSearchKeySelection::RoundRobin => {
            let counter = counters()
                .entry(source_id.to_string())
                .or_insert_with(|| AtomicUsize::new(0));
            let start = counter.fetch_add(1, Ordering::Relaxed) % key_count;
            (0..key_count)
                .map(|offset| (start + offset) % key_count)
                .collect()
        }
        WebSearchKeySelection::Random => {
            let mut order: Vec<usize> = (0..key_count).collect();
            shuffle_indices(&mut order);
            order
        }
    }
}

fn shuffle_indices(order: &mut [usize]) {
    if order.len() <= 1 {
        return;
    }
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xC0FFEE)
        ^ (order.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for i in (1..order.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        let j = (state as usize) % (i + 1);
        order.swap(i, j);
    }
}

#[cfg(test)]
pub fn reset_round_robin_for_tests() {
    if let Some(map) = ROUND_ROBIN_COUNTERS.get() {
        map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::WebSearchKeySelection;

    #[test]
    fn sequential_is_stable() {
        assert_eq!(
            key_attempt_order("s", 3, &WebSearchKeySelection::Sequential),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn round_robin_advances() {
        reset_round_robin_for_tests();
        let first = key_attempt_order("rr-test", 3, &WebSearchKeySelection::RoundRobin);
        let second = key_attempt_order("rr-test", 3, &WebSearchKeySelection::RoundRobin);
        assert_eq!(first, vec![0, 1, 2]);
        assert_eq!(second, vec![1, 2, 0]);
    }

    #[test]
    fn random_contains_all_indices() {
        let order = key_attempt_order("rnd", 4, &WebSearchKeySelection::Random);
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
    }

    #[test]
    fn empty_keys_yield_empty_order() {
        assert!(key_attempt_order("s", 0, &WebSearchKeySelection::Sequential).is_empty());
    }
}
