use std::path::PathBuf;
use zmem_core::SCHEMA_VERSION;
use zmem_core::{AttentionPolicy, TrailIdentity};
use zmem_store::Store;

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zmem-existing-store-{}-{}",
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

#[test]
fn existing_writer_never_creates_or_migrates_a_database() {
    let home = TestDir::new();
    let path = home.0.join("entries.db");
    assert!(Store::open_writable_existing(&path).is_err());
    assert!(!path.exists());

    let mut initial = Store::open(&path).unwrap();
    let repo = initial.register_repository("repo", true).unwrap();
    drop(initial);
    let existing = Store::open_writable_existing(&path).unwrap();
    assert_eq!(existing.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(existing.repository("repo").unwrap(), Some((repo, true)));
    drop(existing);

    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=4").unwrap();
    assert!(Store::open_writable_existing(&path).is_err());
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        4
    );
}

#[test]
fn readonly_lookup_sees_atomic_publication_and_retention() {
    let home = TestDir::new();
    let path = home.0.join("entries.db");
    let mut initial = Store::open(&path).unwrap();
    let repo_id = initial.register_repository("repo", false).unwrap();
    drop(initial);
    let identity = TrailIdentity::new(
        repo_id,
        "head".to_owned(),
        AttentionPolicy::default(),
        "extension",
        5,
        SCHEMA_VERSION,
    );
    let trail_id = format!("{}:view", identity.key());
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute("PRAGMA foreign_keys=ON", []).unwrap();
    writer.execute(
        "INSERT INTO commits(repository_id,oid,commit_time,message) VALUES(?1,'head',1,'message')",
        [repo_id],
    ).unwrap();
    writer.execute(
        "INSERT INTO entries(repository_id,commit_oid,annotation_index,entry_type,content,score,valid,commit_time) VALUES(?1,'head',0,'DECISION','content',1,1,1)",
        [repo_id],
    ).unwrap();
    writer.execute(
        "INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,source_time) VALUES(?1,?2,'head','view','extension',5,?3,1)",
        rusqlite::params![trail_id, repo_id, SCHEMA_VERSION],
    ).unwrap();
    writer.execute(
        "INSERT INTO trail_membership(trail_id,repository_id,commit_oid,position) VALUES(?1,?2,'head',0)",
        rusqlite::params![trail_id, repo_id],
    ).unwrap();
    writer.execute(
        "INSERT INTO trail_entry_state(trail_id,repository_id,commit_oid,annotation_index,entry_type,content,score,valid,commit_time) VALUES(?1,?2,'head',0,'DECISION','content',1,1,1)",
        rusqlite::params![trail_id, repo_id],
    ).unwrap();
    writer.execute(
        "INSERT INTO relationships(repository_id,commit_oid,source,target,score,trail_id) VALUES(?1,'head','head','head',1,?2)",
        rusqlite::params![repo_id, trail_id],
    ).unwrap();
    writer.execute(
        "INSERT INTO diagnostics(repository_id,commit_oid,message,trail_id) VALUES(?1,'head','diagnostic',?2)",
        rusqlite::params![repo_id, trail_id],
    ).unwrap();
    drop(writer);

    let (deleted, reader_done) = std::sync::mpsc::channel();
    let (continue_writer, resume) = std::sync::mpsc::channel();
    let writer_path = path.clone();
    let writer_trail = trail_id.clone();
    let maintenance = std::thread::spawn(move || {
        let connection = rusqlite::Connection::open(&writer_path).unwrap();
        connection.execute("PRAGMA foreign_keys=ON", []).unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        tx.execute(
            "DELETE FROM relationships WHERE trail_id=?1",
            [&writer_trail],
        )
        .unwrap();
        tx.execute("DELETE FROM diagnostics WHERE trail_id=?1", [&writer_trail])
            .unwrap();
        tx.execute("DELETE FROM trails WHERE id=?1", [&writer_trail])
            .unwrap();
        deleted.send(()).unwrap();
        resume.recv().unwrap();
        tx.commit().unwrap();
    });
    reader_done.recv().unwrap();
    let mut reader = Store::open_readonly(&path).unwrap();
    let snapshot = reader
        .published_snapshot(&identity, true, 100, 2, 0)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.relationships.len(), 1);
    assert_eq!(snapshot.diagnostics.len(), 1);
    continue_writer.send(()).unwrap();
    maintenance.join().unwrap();
    assert!(
        reader
            .published_snapshot(&identity, true, 100, 2, 0)
            .unwrap()
            .is_none()
    );

    let writer = rusqlite::Connection::open(&path).unwrap();
    writer.execute("PRAGMA foreign_keys=ON", []).unwrap();
    let publication = writer.unchecked_transaction().unwrap();
    publication.execute(
        "INSERT INTO trails(id,repository_id,head_oid,attention_identity,extension_identity,protocol_version,schema_version,source_time) VALUES(?1,?2,'head','view','extension',5,?3,1)",
        rusqlite::params![trail_id, repo_id, SCHEMA_VERSION],
    ).unwrap();
    publication.execute(
        "INSERT INTO trail_membership(trail_id,repository_id,commit_oid,position) VALUES(?1,?2,'head',0)",
        rusqlite::params![trail_id, repo_id],
    ).unwrap();
    publication.execute(
        "INSERT INTO trail_entry_state(trail_id,repository_id,commit_oid,annotation_index,entry_type,content,score,valid,commit_time) VALUES(?1,?2,'head',0,'DECISION','content',1,1,1)",
        rusqlite::params![trail_id, repo_id],
    ).unwrap();
    publication.execute(
        "INSERT INTO relationships(repository_id,commit_oid,source,target,score,trail_id) VALUES(?1,'head','head','head',1,?2)",
        rusqlite::params![repo_id, trail_id],
    ).unwrap();
    publication.execute(
        "INSERT INTO diagnostics(repository_id,commit_oid,message,trail_id) VALUES(?1,'head','diagnostic',?2)",
        rusqlite::params![repo_id, trail_id],
    ).unwrap();
    assert!(
        reader
            .published_snapshot(&identity, true, 100, 2, 0)
            .unwrap()
            .is_none()
    );
    publication.commit().unwrap();
    let snapshot = reader
        .published_snapshot(&identity, true, 100, 2, 0)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.relationships.len(), 1);
    assert_eq!(snapshot.diagnostics.len(), 1);
}
