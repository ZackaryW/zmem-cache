use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::PathBuf;
use zmem_core::demand::{Observation, RouteDemand};
use zmem_core::{AttentionPolicy, AttentionUsage, GitCommit, TrailIdentity};
use zmem_store::{AdvisoryBatch, DemandUse, RawCommitBatch, Store};

struct Db(PathBuf);
impl Db {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "zmem-adaptive-{}-{}.db",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.0.with_extension("db-shm"));
    }
}
fn fact(oid: &str) -> GitCommit {
    GitCommit {
        sha: oid.into(),
        message: oid.into(),
        commit_time: 1,
        changes: vec![],
    }
}
fn stage(store: &mut Store, repo: i64, oid: &str, quota: u64) -> bool {
    store
        .record_prefetch_batch(
            repo,
            oid,
            1,
            1,
            RawCommitBatch {
                facts: &[fact(oid)],
                parents: &BTreeMap::new(),
            },
            quota,
        )
        .unwrap()
}
fn demand(repo: &str, route: &str) -> RouteDemand {
    RouteDemand {
        repository: repo.into(),
        route: route.into(),
        oid: "head".into(),
        generation: "ext".into(),
        observations: vec![
            Observation {
                at: 100,
                depth: 1000,
            },
            Observation {
                at: 160,
                depth: 1000,
            },
        ],
    }
}

#[test]
fn advisory_limits_and_expiry_never_delete_failures() {
    let db = Db::new();
    let mut store = Store::open(&db.0).unwrap();
    store.insert_index_job("failure", "{}").unwrap();
    store
        .set_index_job_state("failure", "failed", Some("uncertain"))
        .unwrap();
    let routes = (0..5000)
        .map(|i| demand("repo", &format!("refs/heads/{i}")))
        .collect();
    store
        .persist_advisory(&AdvisoryBatch {
            routes,
            uses: vec![],
            now: 160,
        })
        .unwrap();
    assert_eq!(store.load_route_demand(160).unwrap().len(), 4096);
    assert!(store.load_route_demand(4000).unwrap().is_empty());
    assert!(store.load_route_demand(50).unwrap().is_empty());
    let huge = demand(&"x".repeat(9 * 1024 * 1024), "branch");
    store
        .persist_advisory(&AdvisoryBatch {
            routes: vec![huge],
            uses: vec![],
            now: 160,
        })
        .unwrap();
    assert!(store.load_route_demand(160).unwrap().is_empty());
    let jobs = store.recover_index_jobs().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, "failed");
}

#[test]
fn never_used_lru_is_reclaimed_before_used_and_checkpoint_is_invalidated() {
    let db = Db::new();
    let mut store = Store::open(&db.0).unwrap();
    let repo = store.register_repository("repo", false).unwrap();
    assert!(stage(&mut store, repo, "cold", 4096));
    store.set_prefetch_state(repo, "cold", "ready").unwrap();
    assert!(stage(&mut store, repo, "warm", 4096));
    store.set_prefetch_state(repo, "warm", "ready").unwrap();
    let conn = Connection::open(&db.0).unwrap();
    conn.execute(
        "UPDATE speculative_cohorts SET last_demand=100 WHERE head_oid='warm'",
        [],
    )
    .unwrap();
    let used: u64 = conn
        .query_row("SELECT SUM(bytes) FROM raw_commit_facts", [], |r| r.get(0))
        .unwrap();
    assert!(stage(&mut store, repo, "next", used));
    assert!(store.raw_commit(repo, "cold").unwrap().is_none());
    assert!(store.raw_commit(repo, "warm").unwrap().is_some());
    assert_eq!(
        store.prefetch_checkpoint(repo, "cold").unwrap(),
        Some((0, None))
    );
    let metric = store.prefetch_metrics().unwrap();
    assert_eq!(metric.produced_facts, 3);
    assert!(metric.unused_evicted_bytes > 0);
    assert_eq!(metric.reclamation_generation, 1);
}

#[test]
fn actual_trail_reuse_counts_once_and_protects_shared_facts() {
    let db = Db::new();
    let mut store = Store::open(&db.0).unwrap();
    let repo = store.register_repository("repo", false).unwrap();
    assert!(stage(&mut store, repo, "a", 4096));
    store.set_prefetch_state(repo, "a", "ready").unwrap();
    let conn = Connection::open(&db.0).unwrap();
    conn.execute("INSERT INTO commits VALUES(?1,'a',1,'a')", [repo])
        .unwrap();
    conn.execute("INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version) VALUES('trail',?1,'a','view','ext',5,6)",[repo]).unwrap();
    conn.execute(
        "INSERT INTO trail_membership VALUES('trail',?1,'a',0)",
        [repo],
    )
    .unwrap();
    for at in [100, 200] {
        store
            .persist_advisory(&AdvisoryBatch {
                routes: vec![],
                uses: vec![DemandUse {
                    trail: "trail".into(),
                    at,
                }],
                now: at,
            })
            .unwrap();
    }
    let metrics = store.prefetch_metrics().unwrap();
    assert_eq!(metrics.reused_facts, 1);
    assert_eq!(metrics.remaining_unused_bytes, 0);
    assert!(!stage(&mut store, repo, "b", 1));
    assert!(store.raw_commit(repo, "a").unwrap().is_some());
    assert_eq!(
        conn.query_row(
            "SELECT last_demand FROM speculative_cohorts WHERE head_oid='a'",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        200
    );
}

#[test]
fn active_jobs_and_recent_raw_data_block_reclamation() {
    let db = Db::new();
    let mut store = Store::open(&db.0).unwrap();
    let repo = store.register_repository("repo", false).unwrap();
    assert!(stage(&mut store, repo, "a", 4096));
    store.set_prefetch_state(repo, "a", "ready").unwrap();
    store
        .insert_index_job("active", r#"{"path":"repo"}"#)
        .unwrap();
    assert!(!stage(&mut store, repo, "b", 1));
    assert!(store.raw_commit(repo, "a").unwrap().is_some());
    store
        .set_index_job_state("active", "failed", Some("uncertain"))
        .unwrap();
    let conn = Connection::open(&db.0).unwrap();
    conn.execute(
        "UPDATE raw_commit_facts SET fact=json_set(fact,'$.commit_time',unixepoch())",
        [],
    )
    .unwrap();
    store.set_staging_protection(3600);
    assert!(!stage(&mut store, repo, "c", 1));
    assert!(store.raw_commit(repo, "a").unwrap().is_some());
    assert_eq!(store.recover_index_jobs().unwrap()[0].state, "failed");
}

#[test]
fn failed_advisory_migration_rolls_back_without_touching_jobs() {
    let db = Db::new();
    let mut store = Store::open(&db.0).unwrap();
    store.insert_index_job("failure", "{}").unwrap();
    store
        .set_index_job_state("failure", "failed", Some("uncertain"))
        .unwrap();
    drop(store);
    let conn = Connection::open(&db.0).unwrap();
    conn.execute_batch("PRAGMA user_version=5; DROP TABLE route_demand; CREATE VIEW route_demand AS SELECT 1 AS incompatible;").unwrap();
    drop(conn);
    assert!(Store::open(&db.0).is_err());
    let conn = Connection::open(&db.0).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        conn.query_row("SELECT state FROM index_jobs WHERE id='failure'", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
}

#[test]
fn schema_five_upgrade_preserves_exact_view_and_failure_without_synthetic_heat() {
    let db = Db::new();
    let mut store = Store::open(&db.0).unwrap();
    let repo = store.register_repository("repo", false).unwrap();
    store.insert_index_job("failure", "{}").unwrap();
    store
        .set_index_job_state("failure", "failed", Some("uncertain"))
        .unwrap();
    assert!(stage(&mut store, repo, "head", 4096));
    drop(store);
    let conn = Connection::open(&db.0).unwrap();
    let usage = AttentionUsage {
        commit_limit: 500,
        node_limit: 400,
        selected_commits: 1,
        selected_nodes: 0,
        truncated: false,
        reached: vec![],
    };
    let attention = usage.view_identity(None);
    let old = TrailIdentity::new(repo, "head".into(), AttentionPolicy::default(), "ext", 5, 5);
    let id = format!("{}:{attention}", old.key());
    conn.execute("INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version) VALUES(?1,?2,'head',?3,'ext',5,5)",params![id,repo,attention]).unwrap();
    conn.execute("INSERT INTO commits VALUES(?1,'head',1,'head')", [repo])
        .unwrap();
    conn.execute(
        "INSERT INTO trail_membership VALUES(?1,?2,'head',0)",
        params![id, repo],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ref_aliases VALUES(?1,'main',?2,'head')",
        params![repo, id],
    )
    .unwrap();
    conn.execute_batch(
        "PRAGMA user_version=5; DROP TABLE route_demand; CREATE TABLE route_demand(legacy TEXT);",
    )
    .unwrap();
    drop(conn);
    let mut store = Store::open(&db.0).unwrap();
    assert_eq!(store.schema_version().unwrap(), 6);
    let key = TrailIdentity::new(repo, "head".into(), AttentionPolicy::default(), "ext", 5, 6);
    assert!(
        store
            .published_snapshot(&key, false, 100, 0, 0)
            .unwrap()
            .is_some()
    );
    assert!(store.load_route_demand(100).unwrap().is_empty());
    assert_eq!(store.recover_index_jobs().unwrap()[0].state, "failed");
    assert_eq!(
        store.prefetch_checkpoint(repo, "head").unwrap().unwrap().0,
        1
    );
}
