//! Remembering the last answer to a question that takes milliseconds to work
//! out, for settings that move one control at a time.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// A number compared by its bits, for a key: the same bits are the same
/// question whatever they mean, NaN included, so a nonsense setting is
/// worked out once rather than on every look.
#[derive(Clone, Copy, Debug)]
pub struct Exact(pub f64);

impl PartialEq for Exact {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Exact {}

/// The last question asked and its answer. One is enough: everything that
/// asks — the settings screen, the players, the pile-up planner — asks about
/// the settings in force, and they change one control at a time.
///
/// Answers are worked out outside the lock, so two threads asking at once
/// each work theirs out rather than one waiting on the other; the audio
/// thread never asks.
pub struct LastAnswer<K, V> {
    slot: Mutex<Option<(K, V)>>,
}

impl<K: Clone + PartialEq, V: Clone> LastAnswer<K, V> {
    pub const fn new() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    /// The answer for `key`: the remembered one if it was the last asked,
    /// otherwise `work`'s, which is then remembered.
    pub fn get_or(&self, key: K, work: impl FnOnce() -> V) -> V {
        if let Some((last, answer)) = &*self.slot()
            && *last == key
        {
            return answer.clone();
        }
        let answer = work();
        *self.slot() = Some((key, answer.clone()));
        answer
    }

    /// Nothing written here is ever left half-written — every write is one
    /// assignment — so a lock a panic poisoned is still a good one.
    fn slot(&self) -> MutexGuard<'_, Option<(K, V)>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_answer_is_remembered_and_only_the_last() {
        let memo: LastAnswer<Exact, f64> = LastAnswer::new();
        let asked = std::cell::Cell::new(0);
        let ask = |x: f64| {
            memo.get_or(Exact(x), || {
                asked.set(asked.get() + 1);
                x * 2.0
            })
        };
        assert_eq!(ask(1.0), 2.0);
        assert_eq!(ask(1.0), 2.0);
        assert_eq!(asked.get(), 1);
        assert_eq!(ask(3.0), 6.0);
        assert_eq!(ask(1.0), 2.0);
        assert_eq!(asked.get(), 3);
        // NaN is one question, asked once.
        assert!(ask(f64::NAN).is_nan());
        assert!(ask(f64::NAN).is_nan());
        assert_eq!(asked.get(), 4);
    }
}
