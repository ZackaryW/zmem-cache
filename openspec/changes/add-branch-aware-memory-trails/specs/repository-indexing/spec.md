## MODIFIED Requirements

### Requirement: Index only supported zmem annotations
The index SHALL select the newest whole commits reachable from the requested resolved HEAD within the effective commit/node attention policy, apply selected commits parent-before-child into an immutable trail, and persist only entries and relationships emitted by the active supported expander set. Unsupported annotations, DECAY, CANCEL, and META SHALL consume node attention; effects SHALL not consume entry capacity.

#### Scenario: Mixed supported and unsupported annotations
- **WHEN** a selected commit contains one supported entry, one unsupported annotation, and one valid META or lifecycle effect
- **THEN** all annotations consume node attention, only the entry consumes entry capacity, the unsupported annotation produces a diagnostic, and the valid effect applies to trail state

### Requirement: Anchors make fast-forward indexing incremental
Each immutable trail SHALL identify its resolved HEAD, schema version, extension-set identity, effective attention policy, bounded-view identity, and selected membership. When a newly resolved HEAD descends from a compatible retained trail and the effective bounded selection permits reuse, the service SHALL reuse shared facts and incrementally construct a new immutable trail without representing omitted history as indexed.

#### Scenario: Fast-forward branch update
- **WHEN** a branch advances by two commits under a compatible attention view
- **THEN** the service reuses the prior trail's shared facts, indexes the new commits, and atomically exposes a distinct trail for the new HEAD

### Requirement: Invalid anchors trigger repository rebuilds
When no retained trail is compatible with the requested resolved HEAD, attention identity, schema, or extension-set identity, the service SHALL construct a new bounded trail from the selected reachable history. It SHALL NOT delete shared facts or other trails that remain referenced or retained.

#### Scenario: History rewrite removes a cancellation
- **WHEN** a branch rewrite selects history in which a formerly reachable cancellation is absent
- **THEN** the new trail reports the target decision state determined by the rewritten history while the prior trail remains immutable until retention removes it

#### Scenario: Unlimited request follows a bounded trail
- **WHEN** a repository previously has only a default-bounded trail and receives a request with both limits `-1`
- **THEN** the service constructs or reuses a complete-history trail rather than treating the bounded trail as complete

### Requirement: Effects and anchor advancement are atomic
Entry-fact creation, trail membership, DECAY/CANCEL state, META overlays and conflicts, diagnostics, and trail publication for a selected range SHALL commit atomically. Effects SHALL not be stored as queryable entries.

#### Scenario: Indexing fails while applying an effect
- **WHEN** a fatal expansion or effect failure occurs before the trail transaction commits
- **THEN** neither partial target mutations nor a partially published trail become visible
