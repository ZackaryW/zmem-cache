# repository-indexing Specification

## Purpose

Defines deterministic, incremental Git-history indexing into supported zmem entries while preserving correctness across rewrites and effects.

## Requirements

### Requirement: Index only supported zmem annotations
The index SHALL select the newest whole commits reachable from the requested resolved HEAD within the effective commit/node attention policy, apply selected commits parent-before-child into an immutable trail, and persist only entries and relationships emitted by the active supported expander set. Unsupported annotations, DECAY, CANCEL, and META SHALL consume node attention; effects SHALL not consume entry capacity.

#### Scenario: BDD target — Mixed supported and unsupported annotations
- **WHEN** executable behavior is covered by `features/repository-indexing/repository-indexing.feature::Mixed supported and unsupported annotations`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Compatible trails make fast-forward indexing incremental
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

### Requirement: Indexing concurrency is globally bounded
The service SHALL bound simultaneous extension-host work across repositories to `max_concurrency`, defaulting to 8 and accepting a positive override from `~/.zmem/config.toml`. The bound SHALL apply to parser inspection as well as commit expansion, while validated results SHALL be applied in deterministic commit order.

#### Scenario: More work than the configured bound
- **WHEN** indexing exposes more ready host work than `max_concurrency`
- **THEN** simultaneous host execution never exceeds the configured number and application order remains deterministic

### Requirement: Immutable commit inspections are reusable
The service SHALL retain validated parser-inspection results for immutable Git commit identities under the protocol and parser identity that produced them. A later selection SHALL reuse only an exact identity match, SHALL inspect every cache miss through the current host, and SHALL produce the same attention result as fresh inspection. A protocol, parser, or commit identity change SHALL prevent stale inspection reuse.

#### Scenario: Unchanged history is checked again
- **WHEN** a repository command repeats attention selection for commit identities already inspected by the current parser protocol
- **THEN** the command reuses their validated counts without starting parser hosts for those commits

#### Scenario: Parser identity changes
- **WHEN** stored inspection results were produced by a different parser protocol identity
- **THEN** the command ignores those results and obtains current validated inspections before selecting history
