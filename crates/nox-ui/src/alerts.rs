//! Transient island alerts: priority queue with a time-to-live. The island
//! shows one alert at a time; a higher-priority arrival preempts the current one.

use nox_core::{Alert, AlertPriority};

const MAX_QUEUED: usize = 16;

#[derive(Debug, Default)]
pub struct AlertQueue {
    queue: Vec<Alert>,
    current: Option<(Alert, f64)>,
}

impl AlertQueue {
    pub fn push(&mut self, alerts: Vec<Alert>, now: f64) {
        for a in alerts {
            if let Some((cur, _)) = &self.current
                && a.priority > cur.priority
            {
                let (old, _) = self.current.take().expect("checked");
                // A preempted alert that still had a lot of time left comes back.
                if old.priority >= AlertPriority::Normal {
                    self.queue.insert(0, old);
                }
            }
            self.queue.push(a);
        }
        // Highest priority first, FIFO inside a priority (stable sort).
        self.queue.sort_by_key(|a| std::cmp::Reverse(a.priority));
        self.queue.truncate(MAX_QUEUED);
        self.promote(now);
    }

    fn promote(&mut self, now: f64) {
        if self.current.is_none() && !self.queue.is_empty() {
            let a = self.queue.remove(0);
            self.current = Some((a, now));
        }
    }

    /// Expires the current alert if its TTL elapsed and promotes the next one.
    /// Returns true if what is shown changed.
    pub fn tick(&mut self, now: f64) -> bool {
        let mut changed = false;
        while let Some((a, shown)) = &self.current {
            if now >= shown + a.ttl_secs.max(0.5) {
                self.current = None;
                changed = true;
                self.promote(now);
            } else {
                break;
            }
        }
        changed
    }

    pub fn current(&self) -> Option<&Alert> {
        self.current.as_ref().map(|(a, _)| a)
    }

    pub fn dismiss(&mut self, now: f64) {
        self.current = None;
        self.promote(now);
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.current = None;
    }

    pub fn deadline(&self) -> Option<f64> {
        self.current
            .as_ref()
            .map(|(a, shown)| shown + a.ttl_secs.max(0.5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nox_core::AlertKind;

    fn alert(id: u64, p: AlertPriority, ttl: f64) -> Alert {
        let mut a = Alert::new(AlertKind::Info, format!("a{id}"), "bell")
            .priority(p)
            .ttl(ttl);
        a.id = id;
        a
    }

    #[test]
    fn shows_in_turn_with_ttl() {
        let mut q = AlertQueue::default();
        q.push(
            vec![
                alert(1, AlertPriority::Normal, 2.0),
                alert(2, AlertPriority::Normal, 3.0),
            ],
            0.0,
        );
        assert_eq!(q.current().map(|a| a.id), Some(1));
        assert_eq!(q.deadline(), Some(2.0));
        assert!(!q.tick(1.0));
        assert!(q.tick(2.0));
        assert_eq!(q.current().map(|a| a.id), Some(2));
        assert_eq!(q.deadline(), Some(5.0));
        assert!(q.tick(5.0));
        assert!(q.current().is_none());
        assert_eq!(q.deadline(), None, "no timer once empty");
    }

    #[test]
    fn higher_priority_preempts_and_previous_returns() {
        let mut q = AlertQueue::default();
        q.push(vec![alert(1, AlertPriority::Normal, 4.0)], 0.0);
        q.push(vec![alert(2, AlertPriority::High, 4.0)], 1.0);
        assert_eq!(q.current().map(|a| a.id), Some(2));
        q.tick(5.0);
        assert_eq!(q.current().map(|a| a.id), Some(1));
        // Low priority does not preempt.
        q.push(vec![alert(3, AlertPriority::Low, 1.0)], 5.5);
        assert_eq!(q.current().map(|a| a.id), Some(1));
    }

    #[test]
    fn queue_is_bounded() {
        let mut q = AlertQueue::default();
        q.push(
            (0..100)
                .map(|i| alert(i, AlertPriority::Low, 1.0))
                .collect(),
            0.0,
        );
        assert!(q.queue.len() <= MAX_QUEUED);
        q.dismiss(0.1);
        assert_eq!(q.current().map(|a| a.id), Some(1));
    }
}
