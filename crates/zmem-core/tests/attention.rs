use zmem_core::{
    AttentionBound, AttentionCandidate, AttentionLimit, AttentionPolicy,
    attention_identity_allows_incremental, select_attention, validate_host_inspection,
};

#[test]
fn limits_accept_positive_or_unlimited_only() {
    assert_eq!(AttentionLimit::parse(25, "node").unwrap().as_i64(), 25);
    assert_eq!(AttentionLimit::parse(-1, "node").unwrap().as_i64(), -1);
    assert!(AttentionLimit::parse(0, "node").is_err());
    assert!(AttentionLimit::parse(-2, "node").is_err());
}

#[test]
fn request_values_override_environment_independently() {
    let policy = AttentionPolicy::resolve(Some(3), Some(2), Some("1"), Some("1")).unwrap();
    assert_eq!(policy.commit_limit.as_i64(), 3);
    assert_eq!(policy.node_limit.as_i64(), 2);

    let inherited = AttentionPolicy::resolve(None, Some(2), Some("7"), Some("1")).unwrap();
    assert_eq!(inherited.commit_limit.as_i64(), 7);
    assert_eq!(inherited.node_limit.as_i64(), 2);
}

#[test]
fn defaults_are_five_hundred_commits_and_four_hundred_nodes() {
    let policy = AttentionPolicy::resolve(None, None, None, None).unwrap();
    assert_eq!(policy.commit_limit.as_i64(), 500);
    assert_eq!(policy.node_limit.as_i64(), 400);
}

#[test]
fn selector_reserves_proposed_nodes_and_excludes_a_boundary_commit_whole() {
    let policy = AttentionPolicy::resolve(Some(-1), Some(3), None, None).unwrap();
    let selection = select_attention(
        vec![
            AttentionCandidate::new("newest", 1),
            AttentionCandidate::new("boundary", 2),
            AttentionCandidate::new("oldest", 1),
        ],
        policy,
        1,
        false,
    )
    .unwrap();

    assert_eq!(selection.selected, vec!["newest"]);
    assert_eq!(selection.usage.selected_nodes, 2);
    assert!(selection.usage.truncated);
    assert_eq!(selection.usage.reached, vec![AttentionBound::Node]);
}

#[test]
fn selector_reverses_newest_first_candidates_for_replay() {
    let policy = AttentionPolicy::resolve(Some(-1), Some(-1), None, None).unwrap();
    let selection = select_attention(
        vec![
            AttentionCandidate::new("newest", 1),
            AttentionCandidate::new("middle", 0),
            AttentionCandidate::new("oldest", 1),
        ],
        policy,
        0,
        false,
    )
    .unwrap();
    assert_eq!(selection.selected, vec!["oldest", "middle", "newest"]);
    assert!(!selection.usage.truncated);
}

#[test]
fn commit_sentinel_is_reported_even_when_nodes_fit() {
    let policy = AttentionPolicy::resolve(Some(2), Some(-1), None, None).unwrap();
    let selection = select_attention(
        vec![
            AttentionCandidate::new("newest", 0),
            AttentionCandidate::new("older", 0),
        ],
        policy,
        0,
        true,
    )
    .unwrap();
    assert_eq!(selection.usage.reached, vec![AttentionBound::Commit]);
    assert!(selection.usage.truncated);
}

#[test]
fn parser_only_host_inspection_has_a_distinct_validated_shape() {
    let inspection = validate_host_inspection(
        br#"{"protocol_version":2,"annotation_count":3,"parser_diagnostics":["bad annotation"]}"#,
    )
    .unwrap();
    assert_eq!(inspection.annotation_count, 3);
    assert_eq!(inspection.parser_diagnostics, vec!["bad annotation"]);
}

#[test]
fn view_identity_changes_with_the_selected_lower_boundary() {
    let policy = AttentionPolicy::resolve(Some(2), Some(2), None, None).unwrap();
    let selection = select_attention(
        vec![
            AttentionCandidate::new("new", 1),
            AttentionCandidate::new("old", 1),
        ],
        policy,
        0,
        false,
    )
    .unwrap();
    assert_ne!(
        selection.usage.view_identity(Some("old")),
        selection.usage.view_identity(Some("different"))
    );
}

#[test]
fn only_a_complete_identity_under_the_same_policy_allows_incremental_work() {
    let policy = AttentionPolicy::resolve(Some(2), Some(2), None, None).unwrap();
    let complete =
        select_attention(vec![AttentionCandidate::new("new", 1)], policy, 0, false).unwrap();
    let truncated =
        select_attention(vec![AttentionCandidate::new("new", 1)], policy, 0, true).unwrap();
    assert!(attention_identity_allows_incremental(
        &complete.usage.view_identity(Some("new")),
        policy
    ));
    assert!(!attention_identity_allows_incremental(
        &truncated.usage.view_identity(Some("new")),
        policy
    ));
}
