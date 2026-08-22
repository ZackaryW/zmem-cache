use std::collections::BTreeSet;
use zmem_core::{
    EffectiveMetadataValue, ReachableAssignment, TrailCommit, resolve_meta_range,
    resolve_metadata_assignments,
};

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn complete_range_includes_merged_descendants() {
    let membership = vec![
        TrailCommit {
            oid: "a".into(),
            ancestors: set(&[]),
        },
        TrailCommit {
            oid: "b".into(),
            ancestors: set(&["a"]),
        },
        TrailCommit {
            oid: "c".into(),
            ancestors: set(&["a"]),
        },
        TrailCommit {
            oid: "d".into(),
            ancestors: set(&["a", "b", "c"]),
        },
        TrailCommit {
            oid: "meta".into(),
            ancestors: set(&["a", "b", "c", "d"]),
        },
    ];
    assert_eq!(
        resolve_meta_range(&membership, "meta", "a", "d", true).unwrap(),
        vec!["a", "b", "c", "d"]
    );
    assert!(resolve_meta_range(&membership, "meta", "a", "d", false).is_err());
}

#[test]
fn incomparable_values_conflict_until_a_descendant_assignment() {
    let concurrent = vec![
        ReachableAssignment {
            commit_oid: "a".into(),
            value: "one".into(),
            ancestors: set(&[]),
        },
        ReachableAssignment {
            commit_oid: "b".into(),
            value: "two".into(),
            ancestors: set(&[]),
        },
    ];
    assert_eq!(
        resolve_metadata_assignments(&concurrent),
        EffectiveMetadataValue::Conflict(vec!["one".into(), "two".into()])
    );
    let resolved = [
        concurrent[0].clone(),
        concurrent[1].clone(),
        ReachableAssignment {
            commit_oid: "c".into(),
            value: "resolved".into(),
            ancestors: set(&["a", "b"]),
        },
    ];
    assert_eq!(
        resolve_metadata_assignments(&resolved),
        EffectiveMetadataValue::Resolved("resolved".into())
    );
}
