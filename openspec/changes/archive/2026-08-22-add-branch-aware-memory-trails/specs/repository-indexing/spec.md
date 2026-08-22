## MODIFIED Requirements

### Requirement: Index only supported zmem annotations
The index SHALL select the newest whole commits reachable from the requested resolved HEAD within the effective commit/node attention policy, apply selected commits parent-before-child into an immutable trail, and persist only entries and relationships emitted by the active supported expander set. Unsupported annotations, DECAY, CANCEL, and META SHALL consume node attention; effects SHALL not consume entry capacity.

#### Scenario: BDD target — Mixed supported and unsupported annotations
- **WHEN** executable behavior is covered by `features/repository-indexing/repository-indexing.feature::Mixed supported and unsupported annotations`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Anchors make fast-forward indexing incremental
Each immutable trail SHALL identify its resolved HEAD, schema version, extension-set identity, effective attention policy, bounded-view identity, and selected membership. When a newly resolved HEAD descends from a compatible retained trail and the effective bounded selection permits reuse, the service SHALL reuse shared facts and incrementally construct a new immutable trail without representing omitted history as indexed.

#### Scenario: BDD target — Fast-forward branch update
- **WHEN** executable behavior is covered by `features/repository-indexing/repository-indexing.feature::Fast-forward branch update`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Incompatible selections construct new trails
When no retained trail is compatible with the requested resolved HEAD, attention identity, schema, or extension-set identity, the service SHALL construct a new bounded trail from the selected reachable history. It SHALL NOT delete shared facts or other trails that remain referenced or retained.

#### Scenario: BDD target — History rewrite removes a cancellation
- **WHEN** executable behavior is covered by `features/repository-indexing/repository-indexing.feature::History rewrite removes a cancellation`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Unlimited request follows a bounded trail
- **WHEN** executable behavior is covered by `features/repository-indexing/repository-indexing.feature::Unlimited request follows a bounded trail`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Effects and trail publication are atomic
Entry-fact creation, trail membership, DECAY/CANCEL state, META overlays and conflicts, diagnostics, and trail publication for a selected range SHALL commit atomically. Effects SHALL not be stored as queryable entries.

#### Scenario: BDD target — Indexing fails while applying an effect
- **WHEN** executable behavior is covered by `features/repository-indexing/repository-indexing.feature::Indexing fails while applying an effect`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps
