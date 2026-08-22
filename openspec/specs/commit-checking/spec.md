# commit-checking Specification

## Purpose

Defines the sole-writer service behavior for non-persistent fast simulation and isolated historical replay of zmem commit messages and their canonical effects.

## Requirements

### Requirement: Fast checks simulate against synchronized cache state
The service SHALL synchronize the selected repository through its current `HEAD`, expand a proposed successor message with a reserved virtual commit identity, validate its action journal, and simulate its ordered actions against the selected immutable trail without retaining the virtual commit or persisting hypothetical successor state. Synchronizing the real selected history MAY publish a reusable base trail for the effective attention policy.

#### Scenario: Fast cancellation simulation
- **WHEN** a proposed successor contains a valid CANCEL for a cached reachable decision
- **THEN** the service reports the decision's projected invalid state and score `0.0` while the stored decision remains unchanged

### Requirement: Effect simulation reuses canonical semantics
Check evaluation SHALL use the same ordered target resolution and mutation rules as committed indexing. It SHALL report each effect's target, outcome, and before/after score and validity, including diagnostics for ambiguous, unresolved, forward, invalid, or disallowed targets and an explicit no-op outcome when a valid action cannot change current target state.

#### Scenario: Ordered decay and cancellation
- **WHEN** one checked message contains ordered effects against the same earlier decision
- **THEN** their projected outcomes reflect annotation order and the same final state that committed indexing would produce

#### Scenario: Decay after cancellation
- **WHEN** a checked DECAY targets an already invalid decision
- **THEN** the outcome reports a no-op and does not restore validity or score

### Requirement: Deep checks replay into isolated storage
For deep proposed-message checking, the service SHALL reserve node attention for the proposed annotations, select the newest reachable whole commits within the remaining effective commit/node limits, replay those commits parent-before-child into isolated temporary storage using the current active extension set, and then evaluate the proposed message. An existing target commit MAY be evaluated as an additional mode after replaying its selected ancestors and SHALL be applied exactly once. Both limits set to `-1` SHALL select complete reachable history. Deep checking SHALL neither depend on projected persistent-cache rows nor copy projected rows back to the persistent cache.

#### Scenario: Proposed file reveals a bounded deep effect
- **WHEN** a proposed CANCEL targets a decision inside the selected attention view
- **THEN** deep checking reconstructs the decision, reports the projected invalid state, and leaves persistent rows unchanged

#### Scenario: Existing effect commit is not doubled
- **WHEN** deep checking targets an existing DECAY commit
- **THEN** the target's selected ancestors are replayed first and that DECAY is evaluated exactly once

#### Scenario: Target may precede the attention view
- **WHEN** an effect remains unresolved and either attention bound omitted older reachable history
- **THEN** deep checking reports incomplete history rather than a conclusive missing-target rejection

### Requirement: Checks preserve trust and skip hooks
The service SHALL use the repository's persisted extension-trust decision, SHALL request expansion with hooks disabled, and SHALL report the loaded extension identity and skipped-hook state. Check requests SHALL NOT grant or alter repository extension trust.

#### Scenario: Untrusted repository extension
- **WHEN** a check encounters repository extension files for a repository without extension trust
- **THEN** those files are not imported and the check reports the corresponding extension diagnostic

### Requirement: Check responses are structured and non-persistent
The service SHALL return a versioned structured result containing mode, repository, evaluated parent or target, extension identity, actions, effect outcomes, diagnostics, hook state, effective attention limits, observed usage, truncation state, and reached-bound reason. Fatal host, protocol, Git, storage, or attention-validation failures SHALL return a structured service error. Semantic results made inconclusive by omitted history SHALL remain available but unsuccessful. No check SHALL persist its virtual commit, projected actions, projected diagnostics, or projected trail state.

#### Scenario: Persistent state survives failed check
- **WHEN** action-journal validation or proposed-message attention validation fails during a check
- **THEN** the request fails and all previously stored shared facts, trail state, relationships, and diagnostics remain unchanged
