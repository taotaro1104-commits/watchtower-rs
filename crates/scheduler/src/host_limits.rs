use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Semaphore;

/// ホストごとの同時実行数の制限。誰も使っていないセマフォは捨てて、ホスト数に比例して増え続けないようにする。
#[derive(Debug)]
pub struct HostLimits {
    per_host: usize,
    sems: HashMap<String, Arc<Semaphore>>,
}

impl HostLimits {
    pub fn new(per_host: usize) -> Self {
        Self {
            per_host: per_host.max(1),
            sems: HashMap::new(),
        }
    }

    pub fn semaphore(&mut self, host: &str) -> Arc<Semaphore> {
        let per_host = self.per_host;
        Arc::clone(
            self.sems
                .entry(host.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(per_host))),
        )
    }

    /// そのホストのセマフォが使われていなければ捨てる (使われていれば何もしない)。
    pub fn prune(&mut self, host: &str) {
        if self
            .sems
            .get(host)
            .is_some_and(|s| Arc::strong_count(s) == 1)
        {
            self.sems.remove(host);
        }
    }

    /// 使われていないセマフォをすべて捨てる (ホストが分からないとき用)。
    pub fn prune_idle(&mut self) {
        self.sems.retain(|_, s| Arc::strong_count(s) > 1);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sems.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.sems.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unused_semaphore_is_dropped() {
        let mut l = HostLimits::new(2);
        drop(l.semaphore("a.example"));
        assert_eq!(l.len(), 1);
        l.prune("a.example");
        assert!(l.is_empty());
    }

    #[test]
    fn a_semaphore_in_use_is_kept() {
        let mut l = HostLimits::new(2);
        let held = l.semaphore("a.example");
        l.prune("a.example");
        assert_eq!(l.len(), 1);
        drop(held);
        l.prune_idle();
        assert!(l.is_empty());
    }

    #[test]
    fn the_same_host_shares_one_semaphore() {
        let mut l = HostLimits::new(1);
        let (a, b) = (l.semaphore("h"), l.semaphore("h"));
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.available_permits(), 1);
    }
}
