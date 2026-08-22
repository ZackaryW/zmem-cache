use zmem_core::{PROTOCOL_VERSION, validate_action_journal};

#[test]
fn metadata_patch_journal_validates_declared_typed_operations() {
    let payload = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "extension_hash": "extensions",
        "journal": {
            "version": 1,
            "origin": "zmem-expansion-context",
            "actions": [{
                "kind": "metadata_patch",
                "from_sha": "abc123",
                "to_sha": "def456",
                "operations": [
                    {"key": "owner", "operator": "set", "value": "platform"},
                    {"key": "tags", "operator": "add", "value": "security"},
                    {"key": "affected_areas", "operator": "null", "value": null}
                ]
            }]
        },
        "hook_diagnostics": [],
        "annotation_count": 1
    });
    assert!(validate_action_journal(&serde_json::to_vec(&payload).unwrap()).is_ok());
    let invalid = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "extension_hash": "extensions",
        "journal": {"version": 1, "origin": "zmem-expansion-context", "actions": [{
            "kind": "metadata_patch", "from_sha": "a", "to_sha": "b",
            "operations": [{"key": "owner", "operator": "add", "value": "platform"}]
        }]}
    });
    assert!(validate_action_journal(&serde_json::to_vec(&invalid).unwrap()).is_err());
}
