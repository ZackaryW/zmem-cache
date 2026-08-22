use zmem_store::{Cohort, RetentionPolicy, TrailCohort, select_evictions, select_trail_evictions};

#[test]
fn oldest_eligible_cohort_is_selected() {
    let rows = vec![
        Cohort {
            repo_id: 1,
            oid: "new".into(),
            commit_time: 900,
            entries: 2,
        },
        Cohort {
            repo_id: 1,
            oid: "old".into(),
            commit_time: 100,
            entries: 2,
        },
    ];
    let plan = select_evictions(
        &rows,
        1_000,
        RetentionPolicy {
            max_entries: 2,
            protect_recent_seconds: 0,
        },
    );
    assert_eq!(plan.targets, vec![(1, "old".into())]);
    assert!(!plan.over_capacity);
}

#[test]
fn protection_wins_over_capacity() {
    let rows = vec![Cohort {
        repo_id: 1,
        oid: "recent".into(),
        commit_time: 990,
        entries: 5,
    }];
    let plan = select_evictions(
        &rows,
        1_000,
        RetentionPolicy {
            max_entries: 1,
            protect_recent_seconds: 20,
        },
    );
    assert!(plan.targets.is_empty());
    assert!(plan.over_capacity);
}

#[test]
fn ties_are_deterministic() {
    let rows = vec![
        Cohort {
            repo_id: 2,
            oid: "b".into(),
            commit_time: 1,
            entries: 1,
        },
        Cohort {
            repo_id: 1,
            oid: "c".into(),
            commit_time: 1,
            entries: 1,
        },
        Cohort {
            repo_id: 1,
            oid: "a".into(),
            commit_time: 1,
            entries: 1,
        },
    ];
    let plan = select_evictions(
        &rows,
        100,
        RetentionPolicy {
            max_entries: 1,
            protect_recent_seconds: 0,
        },
    );
    assert_eq!(plan.targets, vec![(1, "a".into()), (1, "c".into())]);
}

#[test]
fn unreferenced_trails_are_selected_by_source_time_before_shared_facts() {
    let trails = vec![
        TrailCohort {
            repository_id: 1,
            trail_id: "new".into(),
            source_time: 20,
            referenced: false,
            protected: false,
            entries: 1,
        },
        TrailCohort {
            repository_id: 1,
            trail_id: "live".into(),
            source_time: 1,
            referenced: true,
            protected: false,
            entries: 1,
        },
        TrailCohort {
            repository_id: 1,
            trail_id: "old".into(),
            source_time: 10,
            referenced: false,
            protected: false,
            entries: 1,
        },
    ];
    assert_eq!(select_trail_evictions(&trails), vec!["old", "new"]);
}
