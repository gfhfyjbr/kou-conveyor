use super::*;
use std::{
    collections::{HashMap, VecDeque, hash_map::RandomState},
    hash::BuildHasher,
    sync::Mutex,
};

#[cfg(test)]
mod tests;

/// Key payload and entry limits, not a global allocator/RSS bound. Eviction can
/// forget old call IDs/read parts; a zero limit disables repetition tracking.
#[derive(Clone, Copy, Debug)]
pub struct ReadJournalLimits {
    pub parts: usize,
    pub calls: usize,
    pub key_bytes: usize,
}
impl Default for ReadJournalLimits {
    fn default() -> Self {
        Self {
            parts: 1024,
            calls: 4096,
            key_bytes: 1 << 20,
        }
    }
}
#[derive(Clone, Eq, PartialEq, Hash)]
struct Part {
    path: Arc<str>,
    offset: i64,
    limit: i64,
}
struct Count {
    text: u64,
    count: usize,
}
enum Key {
    Part(Part),
    Call(Arc<str>),
}
#[derive(Default)]
struct State {
    parts: HashMap<Part, Count>,
    calls: HashMap<Arc<str>, usize>,
    order: VecDeque<Key>,
    key_bytes: usize,
}
pub(super) struct Journal {
    limits: ReadJournalLimits,
    hash: RandomState,
    state: Mutex<State>,
}
impl Journal {
    pub(super) fn new(limits: ReadJournalLimits) -> Self {
        Self {
            limits,
            hash: RandomState::new(),
            state: Mutex::new(State::default()),
        }
    }
    pub(super) fn note(&self, call_id: &str, current: &Operation, state: &FileState) -> String {
        if current.status != "completed" || state.action != "read" {
            return String::new();
        }
        let Some(result) = &state.result else {
            return String::new();
        };
        let count = self.count(call_id, state, &result.text);
        if count < 3 {
            return String::new();
        }
        format!(
            "\n\n[harness] These lines of {} have now been read {count} times, unchanged since the first. If you keep coming back to them, write down what you need from them, in your notes if you keep any, and act on it rather than read them again.",
            state.path
        )
    }
    fn count(&self, call_id: &str, part: &FileState, text: &str) -> usize {
        let limits = self.limits;
        if limits.parts == 0 || limits.calls == 0 || limits.key_bytes == 0 {
            return 1;
        }
        let mut state = self.state.lock().unwrap();
        if let Some(count) = state.calls.get(call_id) {
            return *count;
        }
        if part
            .path
            .len()
            .checked_add(call_id.len())
            .is_none_or(|bytes| bytes > limits.key_bytes)
        {
            return 1;
        }
        let key = Part {
            path: Arc::from(part.path.as_str()),
            offset: part.offset,
            limit: part.limit,
        };
        let text = self.hash.hash_one(text);
        let count = state
            .parts
            .get(&key)
            .filter(|prior| prior.text == text)
            .map_or(1, |prior| prior.count.saturating_add(1));
        loop {
            let new_part = !state.parts.contains_key(&key);
            let bytes = call_id.len() + if new_part { key.path.len() } else { 0 };
            if state.parts.len() + usize::from(new_part) <= limits.parts
                && state.calls.len() < limits.calls
                && state
                    .key_bytes
                    .checked_add(bytes)
                    .is_some_and(|total| total <= limits.key_bytes)
            {
                break;
            }
            let Some(oldest) = state.order.pop_front() else {
                return 1;
            };
            match oldest {
                Key::Part(old) => {
                    if state.parts.remove(&old).is_some() {
                        state.key_bytes -= old.path.len();
                    }
                }
                Key::Call(old) => {
                    if state.calls.remove(&old).is_some() {
                        state.key_bytes -= old.len();
                    }
                }
            }
        }
        if !state.parts.contains_key(&key) {
            state.key_bytes += key.path.len();
            state.order.push_back(Key::Part(key.clone()));
        }
        state.parts.insert(key, Count { text, count });
        let id: Arc<str> = Arc::from(call_id);
        state.key_bytes += id.len();
        state.calls.insert(id.clone(), count);
        state.order.push_back(Key::Call(id));
        count
    }
}
