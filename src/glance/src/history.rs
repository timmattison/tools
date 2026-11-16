//! A ring buffer of the last samples.
//!
//! The tool runs for weeks. So the buffer allocates its storage once and never
//! grows after that.

use std::num::NonZeroUsize;

/// A ring buffer of fixed capacity. It never allocates after `new`.
///
/// It keeps the last `capacity` items. A push into a full buffer drops the
/// oldest item.
#[derive(Debug, Clone)]
pub struct History<T> {
    items: Vec<T>,
    capacity: usize,
    /// The index of the oldest item once the storage is full. Before that it is 0.
    start: usize,
}

impl<T: Copy> History<T> {
    /// Makes an empty history that holds `capacity` items.
    #[must_use]
    pub fn new(capacity: NonZeroUsize) -> History<T> {
        History {
            items: Vec::with_capacity(capacity.get()),
            capacity: capacity.get(),
            start: 0,
        }
    }

    /// Adds an item as the newest. A full history drops its oldest item.
    pub fn push(&mut self, item: T) {
        if self.items.len() < self.capacity {
            self.items.push(item);
        } else if let Some(slot) = self.items.get_mut(self.start) {
            *slot = item;
            self.start = (self.start + 1) % self.capacity;
        }
    }

    /// The number of items in the history.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Tells if the history holds no item.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The largest number of items that the history holds.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Gives the items from the oldest to the newest.
    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        let (newer, older) = self.items.split_at(self.start);
        older.iter().chain(newer.iter()).copied()
    }

    /// The item that `push` added last, or `None` when the history is empty.
    #[must_use]
    pub fn newest(&self) -> Option<T> {
        self.iter().last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE: NonZeroUsize = NonZeroUsize::MIN.saturating_add(2);

    fn history_of(items: impl IntoIterator<Item = u32>) -> History<u32> {
        let mut history = History::new(THREE);
        for item in items {
            history.push(item);
        }
        history
    }

    #[test]
    fn an_empty_history_holds_nothing() {
        let history = history_of([]);
        assert_eq!(history.len(), 0);
        assert!(history.is_empty());
        assert_eq!(history.iter().count(), 0);
        assert_eq!(history.newest(), None);
        assert_eq!(history.capacity(), 3);
    }

    #[test]
    fn a_partial_history_keeps_its_items_in_order() {
        let history = history_of([10, 20]);
        assert_eq!(history.iter().collect::<Vec<_>>(), [10, 20]);
        assert_eq!(history.newest(), Some(20));
        assert_eq!(history.len(), 2);
        assert!(!history.is_empty());
    }

    #[test]
    fn a_full_history_keeps_all_items_in_order() {
        let history = history_of([10, 20, 30]);
        assert_eq!(history.iter().collect::<Vec<_>>(), [10, 20, 30]);
    }

    #[test]
    fn a_history_that_wraps_drops_the_oldest_items() {
        let history = history_of([10, 20, 30, 40, 50]);
        assert_eq!(history.iter().collect::<Vec<_>>(), [30, 40, 50]);
        assert_eq!(history.len(), 3);
        assert_eq!(history.newest(), Some(50));
    }

    #[test]
    fn a_history_that_wraps_many_times_keeps_its_size_and_order() {
        let history = history_of(0..1000);
        assert_eq!(history.iter().collect::<Vec<_>>(), [997, 998, 999]);
        assert_eq!(history.len(), history.capacity());
        assert_eq!(history.newest(), Some(999));
    }
}
