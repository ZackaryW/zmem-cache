use std::path::{Path, PathBuf};
use zmem_core::{Action, ActionJournal, Anchor, HostResponse};
use zmem_store::{CommitUpdate, EffectStatus, Store};

struct TestDb(PathBuf);

impl TestDb {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "zmem-store-preview-{}-{unique}.db",
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

fn response(actions: Vec<Action>) -> HostResponse {
    HostResponse {
        protocol_version: 2,
        extension_hash: "extensions".into(),
        journal: ActionJournal {
            version: 1,
            origin: "zmem-expansion-context".into(),
            actions,
        },
        hook_diagnostics: Vec::new(),
        annotation_count: 1,
    }
}

fn anchor(head: &str) -> Anchor {
    Anchor {
        head: head.into(),
        schema: 1,
        extension_hash: "extensions".into(),
        attention_identity: "test-attention".into(),
    }
}

fn seeded_store() -> (TestDb, Store, i64, HostResponse, Anchor) {
    let db = TestDb::new();
    let mut store = Store::open(db.path()).unwrap();
    let repo_id = store.register_repository("repo", false).unwrap();
    let base = response(vec![Action::AddEntry {
        commit_sha: "a".repeat(40),
        annotation_index: 1,
        entry_type: "DECISION".into(),
        content: "keep state".into(),
        score: 1.0,
        valid: true,
        commit_time: 1,
        scope: None,
    }]);
    let base_anchor = anchor(&"a".repeat(40));
    store
        .apply_range(
            repo_id,
            &[CommitUpdate {
                oid: &"a".repeat(40),
                commit_time: 1,
                message: "zmem(DECISION): keep state",
                response: &base,
                anchor: &base_anchor,
                affected_areas: None,
                ancestors: &[],
                range_complete: true,
            }],
            false,
        )
        .unwrap();
    (db, store, repo_id, base, base_anchor)
}

#[test]
fn cancellation_preview_reports_projection_and_rolls_back() {
    let (_db, mut store, repo_id, _base, base_anchor) = seeded_store();
    let preview = response(vec![Action::Cancel {
        target_sha: "a".repeat(8),
        target_index: 1,
    }]);
    let virtual_anchor = anchor(&"0".repeat(40));
    let result = store
        .preview(
            repo_id,
            &CommitUpdate {
                oid: &"0".repeat(40),
                commit_time: 2,
                message: "zmem(CANCEL)[aaaaaaaa, 1]",
                response: &preview,
                anchor: &virtual_anchor,
                affected_areas: None,
                ancestors: &[],
                range_complete: true,
            },
        )
        .unwrap();

    assert_eq!(result.effects[0].status, EffectStatus::Applied);
    assert_eq!(result.effects[0].before_score, Some(1.0));
    assert_eq!(result.effects[0].after_score, Some(0.0));
    assert_eq!(result.effects[0].after_valid, Some(false));
    assert_eq!(
        store.query_entries(repo_id, true).unwrap()[0]["valid"],
        true
    );
    assert_eq!(store.anchor(repo_id).unwrap(), Some(base_anchor));
}

#[test]
fn decay_after_cancel_is_reported_as_no_op() {
    let (_db, mut store, repo_id, _base, _base_anchor) = seeded_store();
    let cancel = response(vec![Action::Cancel {
        target_sha: "a".repeat(8),
        target_index: 1,
    }]);
    let cancel_anchor = anchor(&"b".repeat(40));
    store
        .apply_range(
            repo_id,
            &[CommitUpdate {
                oid: &"b".repeat(40),
                commit_time: 2,
                message: "cancel",
                response: &cancel,
                anchor: &cancel_anchor,
                affected_areas: None,
                ancestors: &[],
                range_complete: true,
            }],
            false,
        )
        .unwrap();
    let decay = response(vec![Action::Decay {
        target_sha: "a".repeat(8),
        target_index: 1,
        factor: 0.5,
    }]);
    let virtual_anchor = anchor(&"0".repeat(40));
    let result = store
        .preview(
            repo_id,
            &CommitUpdate {
                oid: &"0".repeat(40),
                commit_time: 3,
                message: "decay",
                response: &decay,
                anchor: &virtual_anchor,
                affected_areas: None,
                ancestors: &[],
                range_complete: true,
            },
        )
        .unwrap();

    assert_eq!(result.effects[0].status, EffectStatus::NoOp);
    assert_eq!(result.effects[0].before_valid, Some(false));
    assert_eq!(result.effects[0].after_valid, Some(false));
}

#[test]
fn ordered_preview_effects_share_projected_state() {
    let (_db, mut store, repo_id, _base, _base_anchor) = seeded_store();
    let actions = response(vec![
        Action::Decay {
            target_sha: "a".repeat(8),
            target_index: 1,
            factor: 0.5,
        },
        Action::Cancel {
            target_sha: "a".repeat(8),
            target_index: 1,
        },
    ]);
    let virtual_anchor = anchor(&"0".repeat(40));
    let result = store
        .preview(
            repo_id,
            &CommitUpdate {
                oid: &"0".repeat(40),
                commit_time: 2,
                message: "ordered",
                response: &actions,
                anchor: &virtual_anchor,
                affected_areas: None,
                ancestors: &[],
                range_complete: true,
            },
        )
        .unwrap();

    assert_eq!(result.effects[0].after_score, Some(0.5));
    assert_eq!(result.effects[1].before_score, Some(0.5));
    assert_eq!(result.effects[1].after_score, Some(0.0));
}

#[test]
fn unresolved_preview_effect_is_rejected_with_a_diagnostic() {
    let (_db, mut store, repo_id, _base, _base_anchor) = seeded_store();
    let actions = response(vec![Action::Cancel {
        target_sha: "deadbeef".into(),
        target_index: 1,
    }]);
    let virtual_anchor = anchor(&"0".repeat(40));
    let result = store
        .preview(
            repo_id,
            &CommitUpdate {
                oid: &"0".repeat(40),
                commit_time: 2,
                message: "missing",
                response: &actions,
                anchor: &virtual_anchor,
                affected_areas: None,
                ancestors: &[],
                range_complete: true,
            },
        )
        .unwrap();

    assert_eq!(result.effects[0].status, EffectStatus::Rejected);
    assert_eq!(
        result.effects[0].diagnostic.as_deref(),
        Some("unresolved or ambiguous effect target")
    );
    assert_eq!(result.diagnostics.len(), 1);
}
