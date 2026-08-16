## Purpose

Defines the sole-writer service behavior for non-persistent fast simulation and isolated historical replay of zmem commit messages and their canonical effects.

## ADDED Requirements

### Requirement: Fast checks simulate against synchronized cache state
The service SHALL synchronize the selected repository through its current `HEAD`, expand a proposed successor message with a reserved virtual commit identity, validate its action journal, and simulate its ordered actions against the synchronized repository state without retaining the virtual commit or advancing the anchor.

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
For deep checking, the service SHALL replay the target's complete reachable history in parent-before-child order into isolated temporary storage using the current active extension set. A proposed message SHALL follow replay through `HEAD`; an existing target commit SHALL be evaluated after replaying its reachable ancestors and SHALL be applied exactly once. Deep checking SHALL neither depend on projected persistent-cache rows nor copy projected rows back to the persistent cache.

#### Scenario: Evicted target remains deep-checkable
- **WHEN** a reachable target entry is absent from the rolling persistent cache but present in Git history
- **THEN** deep checking reconstructs it and evaluates the proposed effect against it

#### Scenario: Existing effect commit is not doubled
- **WHEN** deep checking targets an existing DECAY commit
- **THEN** the target's ancestors are replayed first and that DECAY is evaluated exactly once

### Requirement: Checks preserve trust and skip hooks
The service SHALL use the repository's persisted extension-trust decision, SHALL request expansion with hooks disabled, and SHALL report the loaded extension identity and skipped-hook state. Check requests SHALL NOT grant or alter repository extension trust.

#### Scenario: Untrusted repository extension
- **WHEN** a check encounters repository extension files for a repository without extension trust
- **THEN** those files are not imported and the check reports the corresponding extension diagnostic

### Requirement: Check responses are structured and non-persistent
The service SHALL return a versioned structured result containing mode, repository, evaluated parent or target, extension identity, actions, effect outcomes, diagnostics, and hook state. Fatal host, protocol, Git, or storage failures SHALL return a structured service error. No check SHALL persist its virtual commit, projected actions, projected diagnostics, or projected anchor.

#### Scenario: Persistent state survives failed check
- **WHEN** action-journal validation fails during a check
- **THEN** the request fails and all previously stored entries, relationships, diagnostics, and anchors remain unchanged
