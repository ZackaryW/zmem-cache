use std::path::{Path, PathBuf};
use zmem_store::{InspectionRecord, Store};

struct TestDb(PathBuf);

impl TestDb {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "zmem-store-inspections-{}-{unique}.db",
            std::process::id()
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

#[test]
fn inspection_cache_is_protocol_keyed_and_atomic() {
    let db = TestDb::new();
    let mut store = Store::open(db.path()).unwrap();
    assert!(store.inspection("abc", 3).unwrap().is_none());
    store
        .record_inspections(
            3,
            &[InspectionRecord {
                oid: "abc".into(),
                annotation_count: 2,
                parser_diagnostics: vec!["diagnostic".into()],
            }],
        )
        .unwrap();
    assert_eq!(
        store
            .inspection("abc", 3)
            .unwrap()
            .unwrap()
            .annotation_count,
        2
    );
    assert!(store.inspection("abc", 4).unwrap().is_none());
}

#[test]
fn schema_two_is_migrated_without_dropping_repository_rows() {
    let db = TestDb::new();
    {
        let connection = rusqlite::Connection::open(db.path()).unwrap();
        connection.execute_batch(
            "PRAGMA user_version=2;
             CREATE TABLE repositories(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,trusted_extensions INTEGER NOT NULL DEFAULT 0);
             INSERT INTO repositories(path,trusted_extensions) VALUES('repo',1);",
        ).unwrap();
    }
    let store = Store::open(db.path()).unwrap();
    assert_eq!(store.repository("repo").unwrap(), Some((1, true)));
    assert!(store.inspection("abc", 3).unwrap().is_none());
}
