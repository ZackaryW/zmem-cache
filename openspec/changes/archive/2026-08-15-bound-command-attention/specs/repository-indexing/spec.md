## MODIFIED Requirements

### Requirement: Index only supported zmem annotations
The index SHALL select the newest whole commits reachable from the requested HEAD within the effective commit/node attention policy, apply selected commits parent-before-child, and persist only entries and relationships emitted by the active supported expander set. Unsupported annotations, DECAY, and CANCEL SHALL consume node attention but SHALL not consume entry capacity.

#### Scenario: Mixed supported and unsupported annotations
- **WHEN** a selected commit contains one supported entry, one unsupported annotation, and one valid effect
- **THEN** all three consume node attention, one entry is stored, the unsupported annotation produces a diagnostic, and the effect is applied

### Requirement: Anchors make fast-forward indexing incremental
Each repository anchor SHALL identify the projected HEAD, schema version, extension-set identity, effective attention policy, and bounded-view identity. If the anchor is an ancestor of the requested HEAD and all identities still match, the service SHALL update the bounded projection incrementally without representing omitted history as indexed.

#### Scenario: Fast-forward update
- **WHEN** a repository advances from its compatible bounded anchor by two commits that fit the attention policy
- **THEN** those new commits are expanded, the bounded view is adjusted, and the anchor advances atomically

### Requirement: Invalid anchors trigger repository rebuilds
If the indexed HEAD is not an ancestor of the requested HEAD, the schema or extension-set identity changed, or the effective attention policy is incompatible with the stored projection, the service SHALL discard that repository's derived state and rebuild the requested bounded view.

#### Scenario: History rewrite removes a cancellation
- **WHEN** the indexed cancellation commit is no longer reachable from the requested HEAD
- **THEN** rebuilding restores the target decision's state as determined by the newly selected history

#### Scenario: Unlimited request follows a bounded anchor
- **WHEN** a repository indexed under default limits is requested with both limits `-1`
- **THEN** the service does not treat the bounded anchor as complete and reconstructs complete reachable history
