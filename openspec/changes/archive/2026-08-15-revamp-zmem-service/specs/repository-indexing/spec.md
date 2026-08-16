## Purpose

Defines deterministic, incremental Git-history indexing into supported zmem entries while preserving correctness across rewrites and effects.

## ADDED Requirements

### Requirement: Index only supported zmem annotations
The index SHALL walk commits reachable from the selected HEAD in parent-before-child order and persist only entries and relationships emitted by the active supported expander set. Unsupported annotations, DECAY, and CANCEL SHALL not consume entry capacity.

#### Scenario: Mixed supported and unsupported annotations
- **WHEN** a commit contains one supported entry, one unsupported annotation, and one valid effect
- **THEN** one entry is stored, the unsupported annotation produces a diagnostic, and the effect is applied

### Requirement: Anchors make fast-forward indexing incremental
Each repository anchor SHALL identify the fully indexed HEAD, schema version, and extension-set identity. If the anchor is an ancestor of the requested HEAD and its identities still match, the service SHALL index only the new reachable range.

#### Scenario: Fast-forward update
- **WHEN** a repository advances from its indexed anchor by two commits
- **THEN** only those new commits are expanded before the anchor advances

### Requirement: Invalid anchors trigger repository rebuilds
If the indexed HEAD is not an ancestor of the requested HEAD, or the schema or extension-set identity changed, the service SHALL discard that repository's derived state and replay its reachable history.

#### Scenario: History rewrite removes a cancellation
- **WHEN** the indexed cancellation commit is no longer reachable from the requested HEAD
- **THEN** rebuilding restores the target decision's state as determined by the new history

### Requirement: Effects and anchor advancement are atomic
Entry creation, DECAY/CANCEL mutations, diagnostics needed for the indexing result, and anchor advancement for a range SHALL commit atomically. Effects SHALL not be stored as entries.

#### Scenario: Indexing fails while applying an effect
- **WHEN** a fatal expansion failure occurs before the range transaction commits
- **THEN** neither target mutations nor the new anchor become visible

### Requirement: Indexing concurrency is globally bounded
The service SHALL bound simultaneous commit-expansion work across repositories to `max_concurrency`, defaulting to 50 and accepting a positive override from `~/.zmem/config.toml`.

#### Scenario: More work than the configured bound
- **WHEN** indexing exposes more ready commits than `max_concurrency`
- **THEN** no more than the configured number are expanded simultaneously while deterministic commit application order is preserved
