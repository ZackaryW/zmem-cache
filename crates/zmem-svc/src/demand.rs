//! Bounded advisory state. Lookup recording uses try_lock and never writes SQL.
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use zmem_core::demand::{MAX_METADATA_BYTES, MAX_ROUTES, RouteDemand};
use zmem_store::{AdvisoryBatch, DemandUse};

struct Tracker {
    routes: BTreeMap<(String, String), RouteDemand>,
    uses: HashMap<String, u64>,
    credited: HashMap<(String, String), Instant>,
    bytes: usize,
    origin: Instant,
    epoch: u64,
    flushed: Instant,
}
static TRACKER: OnceLock<Mutex<Tracker>> = OnceLock::new();
static SKIPPED: AtomicU64 = AtomicU64::new(0);

pub fn initialize(routes: Vec<RouteDemand>) {
    let now = wall_time();
    let mut tracker = Tracker {
        routes: BTreeMap::new(),
        uses: HashMap::new(),
        credited: HashMap::new(),
        bytes: 0,
        origin: Instant::now(),
        epoch: now,
        flushed: Instant::now(),
    };
    for mut route in routes {
        route.expire(now);
        let bytes = size(&route);
        if route.observations.is_empty()
            || tracker.routes.len() >= MAX_ROUTES
            || tracker.bytes + bytes > MAX_METADATA_BYTES / 2
        {
            continue;
        }
        tracker.bytes += bytes;
        tracker
            .routes
            .insert((route.repository.clone(), route.route.clone()), route);
    }
    let _ = TRACKER.set(Mutex::new(tracker));
}
pub fn wall_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn size(route: &RouteDemand) -> usize {
    512 + 3 * (route.repository.len() + route.route.len())
        + route.oid.len()
        + route.generation.len()
        + route.observations.capacity().max(60) * 32
}
pub fn record(summary: &crate::SyncSummary) {
    let Some(tracker) = TRACKER.get() else {
        return;
    };
    let Ok(mut tracker) = tracker.try_lock() else {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let now = tracker.epoch + tracker.origin.elapsed().as_secs();
    let key = (summary.repository.clone(), summary.route.clone());
    let mut route = tracker
        .routes
        .get(&key)
        .cloned()
        .unwrap_or_else(|| RouteDemand {
            repository: key.0.clone(),
            route: key.1.clone(),
            oid: summary.head.clone(),
            generation: summary.trail.extension_identity.clone(),
            observations: Vec::new(),
        });
    let previous_size = tracker.routes.get(&key).map_or(0, size);
    route.oid.clone_from(&summary.head);
    route
        .generation
        .clone_from(&summary.trail.extension_identity);
    let credit = tracker
        .credited
        .get(&key)
        .is_none_or(|at| at.elapsed().as_secs() >= 60);
    let observed = credit && route.observe(now, summary.trail.selected_commits);
    let new_size = size(&route);
    if (previous_size == 0 && tracker.routes.len() >= MAX_ROUTES)
        || tracker.bytes - previous_size + new_size > MAX_METADATA_BYTES / 2
    {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    tracker.bytes = tracker.bytes - previous_size + new_size;
    if observed {
        tracker.credited.insert(key.clone(), Instant::now());
    }
    tracker.routes.insert(key, route);
    let trail = &summary.trail.trail_id;
    if tracker.uses.contains_key(trail) {
        tracker.uses.insert(trail.clone(), now);
    } else if tracker.uses.len() < MAX_ROUTES
        && tracker.bytes + trail.len() + 128 <= MAX_METADATA_BYTES / 2
    {
        tracker.bytes += trail.len() + 128;
        tracker.uses.insert(trail.clone(), now);
    } else {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
    }
}
pub fn skipped() -> u64 {
    SKIPPED.load(Ordering::Relaxed)
}
pub fn targets(ceiling: usize) -> Vec<(RouteDemand, usize)> {
    let Some(tracker) = TRACKER.get() else {
        return Vec::new();
    };
    let Ok(mut tracker) = tracker.lock() else {
        return Vec::new();
    };
    let now = tracker.epoch + tracker.origin.elapsed().as_secs();
    tracker.routes.retain(|_, route| {
        route.expire(now);
        !route.observations.is_empty()
    });
    let keys = tracker
        .routes
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    tracker.credited.retain(|key, _| keys.contains(key));
    tracker.bytes = tracker.routes.values().map(size).sum::<usize>()
        + tracker
            .uses
            .keys()
            .map(|key| key.len() + 128)
            .sum::<usize>();
    if tracker.flushed.elapsed().as_secs() >= 5 {
        let accepted = crate::try_advisory_write(|| AdvisoryBatch {
            routes: tracker.routes.values().cloned().collect(),
            uses: tracker
                .uses
                .iter()
                .map(|(trail, at)| DemandUse {
                    trail: trail.clone(),
                    at: *at,
                })
                .collect(),
            now,
        });
        // Failed admission also observes the batching interval; pending uses
        // stay in the tracker until a later snapshot can be accepted.
        tracker.flushed = Instant::now();
        if accepted {
            tracker.uses.clear();
            tracker.bytes = tracker.routes.values().map(size).sum();
        }
    }
    tracker
        .routes
        .values()
        .filter_map(|route| {
            route
                .target(now, ceiling)
                .map(|target| (route.clone(), target))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lookup_tracking_skips_contention_and_caps_memory_without_sql() {
        initialize(Vec::new());
        let summary = crate::SyncSummary {
            repository: "repo".into(),
            route: "refs/heads/main".into(),
            head: "head".into(),
            indexed_commits: 0,
            entries: 0,
            over_capacity: false,
            max_concurrency: 8,
            attention: zmem_core::AttentionUsage {
                commit_limit: 1000,
                node_limit: 400,
                selected_commits: 1000,
                selected_nodes: 0,
                truncated: true,
                reached: vec![],
            },
            trail: crate::NativeTrailSummary {
                requested_selector: None,
                resolved_oid: "head".into(),
                trail_id: "trail".into(),
                attention_identity: "view".into(),
                selected_commits: 1000,
                selected_nodes: 0,
                extension_identity: "ext".into(),
                protocol_version: 5,
                schema_version: 6,
            },
        };
        let lock = TRACKER.get().unwrap().lock().unwrap();
        let started = Instant::now();
        record(&summary);
        assert!(started.elapsed().as_millis() < 50);
        drop(lock);
        assert_eq!(skipped(), 1);
        record(&summary);
        record(&summary);
        {
            let tracker = TRACKER.get().unwrap().lock().unwrap();
            assert_eq!(
                tracker.routes.values().next().unwrap().observations.len(),
                1
            );
        }
        for index in 0..5000 {
            let mut next = summary.clone();
            next.route = format!("refs/heads/{index}");
            record(&next);
        }
        let tracker = TRACKER.get().unwrap().lock().unwrap();
        assert!(tracker.routes.len() <= MAX_ROUTES);
        assert!(tracker.bytes <= MAX_METADATA_BYTES / 2);
        assert!(skipped() > 1);
    }
}
