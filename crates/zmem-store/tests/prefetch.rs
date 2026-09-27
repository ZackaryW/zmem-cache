use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::PathBuf;
use zmem_core::GitCommit;
use zmem_store::{RawCommitBatch, Store};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zmem-prefetch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fact(sha: &str) -> GitCommit {
    GitCommit {
        sha: sha.to_owned(),
        commit_time: 1,
        message: format!("commit {sha}"),
        changes: Vec::new(),
    }
}

#[test]
fn prefetch_checkpoint_survives_reopen_without_publishing_a_trail() {
    let home = TestDir::new();
    let database = home.0.join("entries.db");
    let repo = home.0.join("repo");
    let mut store = Store::open(&database).unwrap();
    let repo_id = store
        .register_repository(&repo.to_string_lossy(), false)
        .unwrap();
    let first = [fact("a"), fact("b")];
    let parents = BTreeMap::from([("b".to_owned(), vec!["a".to_owned()])]);
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "head",
                3,
                2,
                RawCommitBatch {
                    facts: &first,
                    parents: &parents
                },
                4096,
            )
            .unwrap()
    );
    drop(store);

    let mut resumed = Store::open(&database).unwrap();
    assert_eq!(
        resumed.prefetch_checkpoint(repo_id, "head").unwrap(),
        Some((2, Some("b".into())))
    );
    assert_eq!(
        resumed.pending_prefetch_jobs().unwrap(),
        vec![(repo, "head".into())]
    );
    assert_eq!(
        resumed.raw_commit(repo_id, "a").unwrap().unwrap().message,
        "commit a"
    );
    assert_eq!(resumed.raw_parents(repo_id, "b").unwrap(), vec!["a"]);
    assert!(
        resumed
            .record_prefetch_batch(
                repo_id,
                "head",
                3,
                3,
                RawCommitBatch {
                    facts: &[fact("c")],
                    parents: &BTreeMap::new()
                },
                4096,
            )
            .unwrap()
    );
    resumed
        .set_prefetch_state(repo_id, "head", "ready")
        .unwrap();
    assert!(resumed.pending_prefetch_jobs().unwrap().is_empty());
    assert!(resumed.trails(repo_id).unwrap().is_empty());
}

#[test]
fn quota_pauses_a_batch_without_partial_fact_rows() {
    let home = TestDir::new();
    let mut store = Store::open(&home.0.join("entries.db")).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    assert!(
        !store
            .record_prefetch_batch(
                repo_id,
                "head",
                2,
                2,
                RawCommitBatch {
                    facts: &[fact("a"), fact("b")],
                    parents: &BTreeMap::new()
                },
                1
            )
            .unwrap()
    );
    assert_eq!(
        store.prefetch_checkpoint(repo_id, "head").unwrap(),
        Some((0, None))
    );
    assert!(store.raw_commit(repo_id, "a").unwrap().is_none());
}

#[test]
fn parent_edges_count_against_staging_quota_atomically() {
    let home = TestDir::new();
    let mut store = Store::open(&home.0.join("entries.db")).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    let facts = [fact("a"), fact("b")];
    let parents = BTreeMap::from([("b".to_owned(), vec!["a".to_owned()])]);
    let fact_bytes: usize = facts
        .iter()
        .map(|fact| serde_json::to_string(fact).unwrap().len())
        .sum();
    assert!(
        !store
            .record_prefetch_batch(
                repo_id,
                "head",
                2,
                2,
                RawCommitBatch {
                    facts: &facts,
                    parents: &parents,
                },
                fact_bytes as u64,
            )
            .unwrap()
    );
    assert!(store.raw_commit(repo_id, "a").unwrap().is_none());
    assert!(store.raw_parents(repo_id, "b").unwrap().is_empty());
}

#[test]
fn retained_trail_facts_do_not_exhaust_staging_quota() {
    let home = TestDir::new();
    let database = home.0.join("entries.db");
    let mut store = Store::open(&database).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO commits(repository_id,oid,commit_time,message) VALUES(?1,'a',1,'a')",
            [repo_id],
        )
        .unwrap();
    connection.execute(
        "INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version)
         VALUES('retained',?1,'a','view','extension',5,5)",
        [repo_id],
    ).unwrap();
    connection
        .execute(
            "INSERT INTO trail_membership(trail_id,repository_id,commit_oid,position)
         VALUES('retained',?1,'a',0)",
            [repo_id],
        )
        .unwrap();
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "a",
                1,
                1,
                RawCommitBatch {
                    facts: &[fact("a")],
                    parents: &BTreeMap::new()
                },
                1,
            )
            .unwrap()
    );
    assert!(store.raw_commit(repo_id, "a").unwrap().is_some());
    assert!(
        !store
            .record_prefetch_batch(
                repo_id,
                "b",
                1,
                1,
                RawCommitBatch {
                    facts: &[fact("b")],
                    parents: &BTreeMap::new()
                },
                1,
            )
            .unwrap()
    );
    assert!(store.raw_commit(repo_id, "b").unwrap().is_none());
}

#[test]
fn obsolete_job_reclaims_only_unretained_raw_facts() {
    let home = TestDir::new();
    let database = home.0.join("entries.db");
    let mut store = Store::open(&database).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    let parents = BTreeMap::from([("b".to_owned(), vec!["a".to_owned()])]);
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "b",
                2,
                2,
                RawCommitBatch {
                    facts: &[fact("b"), fact("a")],
                    parents: &parents
                },
                4096,
            )
            .unwrap()
    );
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO commits(repository_id,oid,commit_time,message) VALUES(?1,'a',1,'a')",
            [repo_id],
        )
        .unwrap();
    connection.execute(
        "INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version)
         VALUES('retained',?1,'a','view','extension',5,5)",
        [repo_id],
    ).unwrap();
    connection.execute(
        "INSERT INTO trail_membership(trail_id,repository_id,commit_oid,position) VALUES('retained',?1,'a',0)",
        [repo_id],
    ).unwrap();
    store.set_prefetch_state(repo_id, "b", "obsolete").unwrap();
    assert!(store.raw_commit(repo_id, "a").unwrap().is_some());
    assert!(store.raw_commit(repo_id, "b").unwrap().is_none());
    assert!(store.raw_parents(repo_id, "b").unwrap().is_empty());
}

#[test]
fn full_quota_reclaims_completed_unpinned_prefetch() {
    let home = TestDir::new();
    let mut store = Store::open(&home.0.join("entries.db")).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    let quota = serde_json::to_string(&fact("b")).unwrap().len() as u64;
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "a",
                1,
                1,
                RawCommitBatch {
                    facts: &[fact("a")],
                    parents: &BTreeMap::new()
                },
                4096,
            )
            .unwrap()
    );
    store.set_prefetch_state(repo_id, "a", "ready").unwrap();
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "b",
                1,
                1,
                RawCommitBatch {
                    facts: &[fact("b")],
                    parents: &BTreeMap::new()
                },
                quota,
            )
            .unwrap()
    );
    assert!(store.raw_commit(repo_id, "a").unwrap().is_none());
    assert!(store.raw_commit(repo_id, "b").unwrap().is_some());
    assert_eq!(
        store.prefetch_checkpoint(repo_id, "a").unwrap(),
        Some((0, None))
    );
}

#[test]
fn persisted_prefetch_pins_are_bounded() {
    let home = TestDir::new();
    let database = home.0.join("entries.db");
    let mut store = Store::open(&database).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    for index in 0..33 {
        let oid = format!("{index:040x}");
        assert!(
            store
                .record_prefetch_batch(
                    repo_id,
                    &oid,
                    1,
                    1,
                    RawCommitBatch {
                        facts: &[fact(&oid)],
                        parents: &BTreeMap::new()
                    },
                    4096 * 33,
                )
                .unwrap()
        );
    }
    let connection = Connection::open(&database).unwrap();
    let active: i64 = connection.query_row(
        "SELECT COUNT(*) FROM prefetch_jobs WHERE state IN ('running','paused_shutdown','paused_capacity')",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(active, 32);
    assert!(
        store
            .raw_commit(repo_id, &format!("{:040x}", 0))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .raw_commit(repo_id, &format!("{:040x}", 32))
            .unwrap()
            .is_some()
    );
}

#[test]
fn restart_reconciliation_removes_orphaned_staging() {
    let home = TestDir::new();
    let database = home.0.join("entries.db");
    let mut store = Store::open(&database).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "a",
                1,
                1,
                RawCommitBatch {
                    facts: &[fact("a")],
                    parents: &BTreeMap::new()
                },
                4096,
            )
            .unwrap()
    );
    Connection::open(&database)
        .unwrap()
        .execute(
            "DELETE FROM prefetch_jobs WHERE repository_id=?1",
            [repo_id],
        )
        .unwrap();
    drop(store);
    let mut restarted = Store::open(&database).unwrap();
    restarted.reconcile_prefetch_staging().unwrap();
    assert!(restarted.raw_commit(repo_id, "a").unwrap().is_none());
}

#[test]
fn full_quota_can_release_an_older_paused_speculation() {
    let home = TestDir::new();
    let mut store = Store::open(&home.0.join("entries.db")).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "a",
                1,
                1,
                RawCommitBatch {
                    facts: &[fact("a")],
                    parents: &BTreeMap::new()
                },
                4096,
            )
            .unwrap()
    );
    store
        .set_prefetch_state(repo_id, "a", "paused_capacity")
        .unwrap();
    let quota = serde_json::to_string(&fact("b")).unwrap().len() as u64;
    assert!(
        store
            .record_prefetch_batch(
                repo_id,
                "b",
                1,
                1,
                RawCommitBatch {
                    facts: &[fact("b")],
                    parents: &BTreeMap::new()
                },
                quota,
            )
            .unwrap()
    );
    assert!(store.raw_commit(repo_id, "a").unwrap().is_none());
    assert!(store.raw_commit(repo_id, "b").unwrap().is_some());
}
