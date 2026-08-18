# repository-indexing Specification

## Purpose

Defines deterministic, incremental Git-history indexing into supported zmem entries while preserving correctness across rewrites and effects.

## Requirements

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

### Requirement: Effects and anchor advancement are atomic
Entry creation, DECAY/CANCEL mutations, diagnostics needed for the indexing result, and anchor advancement for a range SHALL commit atomically. Effects SHALL not be stored as entries.

#### Scenario: Indexing fails while applying an effect
- **WHEN** a fatal expansion failure occurs before the range transaction commits
- **THEN** neither target mutations nor the new anchor become visible

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
