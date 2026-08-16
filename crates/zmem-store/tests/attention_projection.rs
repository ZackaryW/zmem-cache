use std::path::{Path, PathBuf};
use zmem_core::{Action, ActionJournal, Anchor, HostResponse};
use zmem_store::{CommitUpdate, Store};

struct TestDb(PathBuf);

impl TestDb {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "zmem-store-attention-{}-{unique}.db",
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

fn response(oid: &str, content: &str) -> HostResponse {
    HostResponse {
        protocol_version: 2,
        extension_hash: "extensions".into(),
        journal: ActionJournal {
            version: 1,
            origin: "zmem-expansion-context".into(),
            actions: vec![Action::AddEntry {
                commit_sha: oid.into(),
                annotation_index: 1,
                entry_type: "DECISION".into(),
                content: content.into(),
                score: 1.0,
                valid: true,
                commit_time: 1,
                scope: None,
            }],
        },
        hook_diagnostics: Vec::new(),
        annotation_count: 1,
    }
}

fn anchor(head: &str, attention_identity: &str) -> Anchor {
    Anchor {
        head: head.into(),
        schema: 1,
        extension_hash: "extensions".into(),
        attention_identity: attention_identity.into(),
    }
}

#[test]
fn replacing_a_bounded_projection_removes_stale_rows_and_sets_final_anchor() {
    let db = TestDb::new();
    let mut store = Store::open(db.path()).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    let old_oid = "a".repeat(40);
    let old_response = response(&old_oid, "old");
    let old_anchor = anchor(&old_oid, "bounded-old");
    store
        .replace_projection(
            repo_id,
            &[CommitUpdate {
                oid: &old_oid,
                commit_time: 1,
                message: "zmem(DECISION): old",
                response: &old_response,
                anchor: &old_anchor,
            }],
            &old_anchor,
        )
        .unwrap();

    let new_oid = "b".repeat(40);
    let new_response = response(&new_oid, "new");
    let new_anchor = anchor(&new_oid, "bounded-new");
    store
        .replace_projection(
            repo_id,
            &[CommitUpdate {
                oid: &new_oid,
                commit_time: 2,
                message: "zmem(DECISION): new",
                response: &new_response,
                anchor: &new_anchor,
            }],
            &new_anchor,
        )
        .unwrap();

    let entries = store.query_entries(repo_id, true).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["content"], "new");
    assert_eq!(store.anchor(repo_id).unwrap(), Some(new_anchor));
}
