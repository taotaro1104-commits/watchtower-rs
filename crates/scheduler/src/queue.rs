use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use watchtower_core::WatchId;

/// 期限順の優先度キュー。ウォッチごとのタイマーは持たない。
/// 再スケジュール・削除は遅延削除 (古いエントリは取り出し時に捨てる)。
#[derive(Debug)]
pub struct DueQueue<T: Ord + Copy> {
    heap: BinaryHeap<Reverse<(T, WatchId)>>,
    current: HashMap<WatchId, T>,
}

impl<T: Ord + Copy> Default for DueQueue<T> {
    fn default() -> Self {
        Self {
            heap: BinaryHeap::new(),
            current: HashMap::new(),
        }
    }
}

impl<T: Ord + Copy> DueQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.current.len()
    }

    pub fn is_empty(&self) -> bool {
        self.current.is_empty()
    }

    pub fn schedule(&mut self, id: WatchId, at: T) {
        self.current.insert(id, at);
        self.heap.push(Reverse((at, id)));
    }

    pub fn remove(&mut self, id: WatchId) {
        self.current.remove(&id);
    }

    fn is_live(&self, at: T, id: WatchId) -> bool {
        self.current.get(&id) == Some(&at)
    }

    /// 次の期限。
    pub fn next_due(&mut self) -> Option<T> {
        while let Some(Reverse((at, id))) = self.heap.peek().copied() {
            if self.is_live(at, id) {
                return Some(at);
            }
            self.heap.pop();
        }
        None
    }

    /// `now` までに期限が来たウォッチを 1 件取り出す。
    pub fn pop_due(&mut self, now: T) -> Option<WatchId> {
        let at = self.next_due()?;
        if at > now {
            return None;
        }
        let Reverse((_, id)) = self.heap.pop()?;
        self.current.remove(&id);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn pops_in_deadline_order() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut q = DueQueue::new();
        q.schedule(a, 30);
        q.schedule(b, 10);
        q.schedule(c, 20);
        assert_eq!(q.next_due(), Some(10));
        assert_eq!(q.pop_due(100), Some(b));
        assert_eq!(q.pop_due(100), Some(c));
        assert_eq!(q.pop_due(100), Some(a));
        assert_eq!(q.pop_due(100), None);
    }

    #[test]
    fn does_not_pop_future_entries() {
        let a = Uuid::new_v4();
        let mut q = DueQueue::new();
        q.schedule(a, 50);
        assert_eq!(q.pop_due(49), None);
        assert_eq!(q.pop_due(50), Some(a));
    }

    #[test]
    fn reschedule_replaces_old_entry() {
        let a = Uuid::new_v4();
        let mut q = DueQueue::new();
        q.schedule(a, 10);
        q.schedule(a, 90);
        assert_eq!(q.next_due(), Some(90));
        assert_eq!(q.pop_due(50), None);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn removed_entries_are_skipped() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut q = DueQueue::new();
        q.schedule(a, 1);
        q.schedule(b, 2);
        q.remove(a);
        assert_eq!(q.pop_due(10), Some(b));
        assert!(q.is_empty());
    }
}
