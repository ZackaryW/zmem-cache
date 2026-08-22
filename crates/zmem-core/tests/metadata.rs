use zmem_core::{ChangedPath, derive_affected_areas};

#[test]
fn compact_areas_cover_root_common_parents_and_rename_endpoints() {
    let changes = vec![
        ChangedPath::path("README.md"),
        ChangedPath::path("a/one/file.rs"),
        ChangedPath::path("a/two/file.rs"),
        ChangedPath::rename("b/old", "b/sub/new"),
    ];
    assert_eq!(
        derive_affected_areas(&changes),
        Some(vec!["<root>".into(), "a".into(), "b".into()])
    );
}

#[test]
fn one_deep_subtree_keeps_its_parent() {
    assert_eq!(
        derive_affected_areas(&[ChangedPath::path("b/sub/file.rs")]),
        Some(vec!["b/sub".into()])
    );
}

#[test]
fn four_compact_areas_become_global() {
    let changes = ["a/x", "b/x", "c/x", "d/x"].map(ChangedPath::path);
    assert_eq!(derive_affected_areas(&changes), None);
}

#[test]
fn deletions_retain_the_deleted_path_parent() {
    assert_eq!(
        derive_affected_areas(&[ChangedPath::delete("a/removed/file.rs")]),
        Some(vec!["a/removed".into()])
    );
}
