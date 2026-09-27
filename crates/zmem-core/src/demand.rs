//! Advisory demand policy. Callers supply elapsed seconds from a monotonic
//! clock; persisted timestamps are validated against wall time at restart.
use serde::{Deserialize, Serialize};

pub const WINDOW: usize = 500;
pub const SAMPLE_SECONDS: u64 = 60;
pub const EXPIRY_SECONDS: u64 = 3600;
pub const MAX_ROUTES: usize = 4096;
pub const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub at: u64,
    pub depth: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteDemand {
    pub repository: String,
    pub route: String,
    pub oid: String,
    pub generation: String,
    pub observations: Vec<Observation>,
}

impl RouteDemand {
    pub fn expire(&mut self, now: u64) {
        self.observations
            .retain(|sample| sample.at <= now && now - sample.at < EXPIRY_SECONDS);
        self.observations.sort_by_key(|sample| sample.at);
        let mut last = None;
        self.observations.retain(|sample| {
            if last.is_some_and(|at| sample.at - at < SAMPLE_SECONDS) {
                return false;
            }
            last = Some(sample.at);
            true
        });
        if self.observations.len() > 60 {
            self.observations.drain(..self.observations.len() - 60);
        }
    }

    pub fn observe(&mut self, now: u64, depth: usize) -> bool {
        self.expire(now);
        if self
            .observations
            .last()
            .is_some_and(|sample| now - sample.at < SAMPLE_SECONDS)
        {
            return false;
        }
        self.observations.push(Observation { at: now, depth });
        true
    }

    pub fn target(&self, now: u64, ceiling: usize) -> Option<usize> {
        let mut deeper = self.observations.iter().filter(|sample| {
            sample.at <= now && now - sample.at < EXPIRY_SECONDS && sample.depth > WINDOW
        });
        let first = deeper.next()?;
        let second = deeper.next()?;
        let depth = deeper.fold(first.depth.max(second.depth), |depth, sample| {
            depth.max(sample.depth)
        });
        let target = depth.saturating_add(WINDOW).min(ceiling);
        (target > depth).then_some(target)
    }

    pub fn revision(&self) -> u64 {
        self.observations.last().map_or(0, |sample| sample.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route() -> RouteDemand {
        RouteDemand {
            repository: "repo".into(),
            route: "refs/heads/main".into(),
            oid: "head".into(),
            generation: "extension".into(),
            observations: Vec::new(),
        }
    }

    #[test]
    fn shallow_and_annotation_limited_demand_never_promote() {
        let mut demand = route();
        for time in 0..120 {
            demand.observe(time * 60, if time % 2 == 0 { 500 } else { 120 });
            assert_eq!(demand.target(time * 60, 10_000), None);
            assert!(demand.observations.len() <= 60);
        }
    }

    #[test]
    fn separate_intervals_justify_only_one_lead_window() {
        let mut demand = route();
        assert!(demand.observe(1, 1000));
        assert!(!demand.observe(60, 9000));
        assert_eq!(demand.target(60, 10_000), None);
        assert!(demand.observe(61, 1000));
        assert_eq!(demand.target(61, 10_000), Some(1500));
        for now in (121..3600).step_by(60) {
            demand.observe(now, 1000);
            assert_eq!(demand.target(now, 10_000), Some(1500));
        }
        assert_eq!(demand.target(3600, 0), None);
        assert_eq!(demand.target(3600, 1200), Some(1200));
        assert_eq!(demand.target(7200, 10_000), None);
    }

    #[test]
    fn unlimited_selected_depth_does_not_authorize_beyond_ceiling() {
        let mut demand = route();
        demand.observe(0, 20_000);
        demand.observe(60, 20_000);
        assert_eq!(demand.target(60, 10_000), None);
        demand.expire(30); // persisted future observations after clock rollback
        assert_eq!(demand.observations.len(), 1);
        assert_eq!(demand.target(30, 10_000), None);
    }
}
