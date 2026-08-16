use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use zmem_core::{
    Anchor, SyncDecision, run_ordered, select_sync, validate_action_journal, validate_factor,
};

#[test]
fn matching_ancestor_is_incremental() {
    let anchor = Anchor {
        head: "old".into(),
        schema: 1,
        extension_hash: "x".into(),
        attention_identity: "attention".into(),
    };
    assert_eq!(
        select_sync(Some(&anchor), "new", 1, "x", "attention", true),
        SyncDecision::Incremental
    );
}

#[test]
fn identity_change_rebuilds() {
    let anchor = Anchor {
        head: "old".into(),
        schema: 1,
        extension_hash: "x".into(),
        attention_identity: "attention".into(),
    };
    assert_eq!(
        select_sync(Some(&anchor), "new", 1, "y", "attention", true),
        SyncDecision::Rebuild
    );
}

#[test]
fn current_head_is_current() {
    let anchor = Anchor {
        head: "same".into(),
        schema: 1,
        extension_hash: "x".into(),
        attention_identity: "attention".into(),
    };
    assert_eq!(
        select_sync(Some(&anchor), "same", 1, "x", "attention", true),
        SyncDecision::Current
    );
}

#[test]
fn factor_range_is_closed_unit_interval() {
    assert!(validate_factor(0.0));
    assert!(validate_factor(1.0));
    assert!(!validate_factor(-0.1));
    assert!(!validate_factor(f64::NAN));
}

#[test]
fn unjournaled_response_is_rejected() {
    let raw = br#"{"protocol_version":2,"extension_hash":"x","entries":[],"effects":[],"relationships":[],"diagnostics":[]}"#;
    assert!(validate_action_journal(raw).is_err());
}

#[test]
fn bounded_work_preserves_input_order() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let active_worker = Arc::clone(&active);
    let peak_worker = Arc::clone(&peak);
    let results = run_ordered(vec![3, 2, 1, 0], 2, move |value| {
        let now = active_worker.fetch_add(1, Ordering::SeqCst) + 1;
        peak_worker.fetch_max(now, Ordering::SeqCst);
        std::thread::yield_now();
        active_worker.fetch_sub(1, Ordering::SeqCst);
        value * 2
    });
    assert_eq!(results, vec![6, 4, 2, 0]);
    assert!(peak.load(Ordering::SeqCst) <= 2);
}
