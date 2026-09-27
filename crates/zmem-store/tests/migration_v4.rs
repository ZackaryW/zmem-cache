use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zmem_store::Store;

static NEXT_DB: AtomicU64 = AtomicU64::new(0);

struct TestDb(PathBuf);

impl TestDb {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "zmem-store-v4-{}-{}-{}.db",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("db-shm"));
        let _ = std::fs::remove_file(self.0.with_extension("db-wal"));
    }
}

fn create_schema_three(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=3;
         CREATE TABLE repositories(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,trusted_extensions INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE anchors(repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,head TEXT NOT NULL,schema_version INTEGER NOT NULL,extension_hash TEXT NOT NULL,attention_identity TEXT NOT NULL);
         CREATE TABLE commits(repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,oid TEXT NOT NULL,commit_time INTEGER NOT NULL,message TEXT NOT NULL,PRIMARY KEY(repository_id,oid));
         CREATE TABLE entries(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,annotation_index INTEGER NOT NULL,entry_type TEXT NOT NULL,content TEXT NOT NULL,score REAL NOT NULL,valid INTEGER NOT NULL,commit_time INTEGER NOT NULL DEFAULT 0,scope TEXT,PRIMARY KEY(repository_id,commit_oid,annotation_index),FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
         CREATE TABLE relationships(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,source TEXT NOT NULL,target TEXT NOT NULL,score REAL NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
         CREATE TABLE diagnostics(repository_id INTEGER NOT NULL,commit_oid TEXT NOT NULL,message TEXT NOT NULL,FOREIGN KEY(repository_id,commit_oid) REFERENCES commits(repository_id,oid) ON DELETE CASCADE);
         CREATE TABLE inspections(commit_oid TEXT NOT NULL,parser_protocol INTEGER NOT NULL,annotation_count INTEGER NOT NULL,parser_diagnostics TEXT NOT NULL,PRIMARY KEY(commit_oid,parser_protocol));"
    ).unwrap();
    connection
}

#[test]
fn schema_three_projection_becomes_a_global_legacy_trail_without_replay() {
    let db = TestDb::new();
    let connection = create_schema_three(db.path());
    let oid = "a".repeat(40);
    connection
        .execute(
            "INSERT INTO repositories(id,path,trusted_extensions) VALUES(1,'repo',1)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO commits VALUES(1,?1,10,'zmem(DECISION): legacy')",
            [&oid],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO entries VALUES(1,?1,1,'DECISION','legacy',0.4,0,10,'core')",
            [&oid],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO relationships VALUES(1,?1,'left','right',0.8)",
            [&oid],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO diagnostics VALUES(1,?1,'legacy diagnostic')",
            [&oid],
        )
        .unwrap();
    connection
        .execute("INSERT INTO anchors VALUES(1,?1,3,'ext','bounded')", [&oid])
        .unwrap();
    drop(connection);

    let store = Store::open(db.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 6);
    assert_eq!(store.repository("repo").unwrap(), Some((1, true)));
    let trails = store.trails(1).unwrap();
    assert_eq!(trails.len(), 1);
    assert!(trails[0].legacy);
    assert_eq!(trails[0].head_oid, oid);
    let entries = store.query_trail_entries(&trails[0].id, true).unwrap();
    assert_eq!(entries[0]["score"], 0.4);
    assert_eq!(entries[0]["valid"], false);
    assert_eq!(entries[0]["affected_areas"], serde_json::Value::Null);
}

#[test]
fn failed_schema_three_migration_rolls_back_the_version() {
    let db = TestDb::new();
    let connection = create_schema_three(db.path());
    connection
        .execute("CREATE TABLE trails(id TEXT PRIMARY KEY)", [])
        .unwrap();
    drop(connection);
    assert!(Store::open(db.path()).is_err());
    let connection = Connection::open(db.path()).unwrap();
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

fn create_schema_four_with_selection(path: &Path) -> String {
    let mut store = Store::open(path).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    drop(store);
    let oid = "b".repeat(40);
    let connection = Connection::open(path).unwrap();
    connection.execute(
        "INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,selected_commit_count,selected_node_count)
         VALUES('selected',?1,?2,'view:complete','extension',4,4,7,3)",
        rusqlite::params![repo_id, oid],
    ).unwrap();
    connection
        .execute_batch(
            "DROP TABLE index_jobs;
         DROP TABLE prefetch_jobs;
         DROP TABLE raw_parent_edges;
         DROP TABLE raw_commit_facts;
         DROP TABLE commit_parents;
         PRAGMA user_version=4;",
        )
        .unwrap();
    oid
}

#[test]
fn schema_four_migration_keeps_published_selection_metadata() {
    let db = TestDb::new();
    let oid = create_schema_four_with_selection(db.path());
    let store = Store::open(db.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 6);
    let trail = &store.trails(1).unwrap()[0];
    assert_eq!(trail.head_oid, oid);
    assert_eq!(trail.attention_identity, "view:complete");
    assert_eq!(trail.selected_commit_count, 7);
    assert_eq!(trail.selected_node_count, 3);
    let connection = Connection::open(db.path()).unwrap();
    for table in [
        "commit_parents",
        "raw_commit_facts",
        "raw_parent_edges",
        "prefetch_jobs",
        "index_jobs",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "missing {table}");
    }
}

#[test]
fn failed_schema_four_migration_keeps_previous_version() {
    let db = TestDb::new();
    create_schema_four_with_selection(db.path());
    let connection = Connection::open(db.path()).unwrap();
    connection.execute_batch(
        "CREATE VIEW commit_parents AS SELECT 1 AS repository_id, 'a' AS commit_oid, 'b' AS parent_oid;"
    ).unwrap();
    drop(connection);
    assert!(Store::open(db.path()).is_err());
    let connection = Connection::open(db.path()).unwrap();
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);
    let table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='raw_commit_facts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_count, 0);
}
