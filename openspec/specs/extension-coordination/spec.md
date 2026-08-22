# extension-coordination Specification

## Purpose

Defines how the Rust service obtains deterministic derived data from the trusted Python extension host without giving up database authority.

## Requirements

### Requirement: Service validates extension-host exchanges
The service SHALL exchange protocol-v4 typed messages with the extension host and reject incompatible, malformed, or non-deterministically identified entry, relationship, DECAY, CANCEL, META, or diagnostic responses before committing shared facts or trail state.

#### Scenario: BDD target — Protocol version mismatch
- **WHEN** executable behavior is covered by `features/extension-coordination/extension-coordination.feature::Protocol version mismatch`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Service remains the only canonical writer
The extension host SHALL return a typed journal of entry, relationship, decay, cancel, metadata-patch, and diagnostic actions performed through expander contexts, together with hook diagnostics and an extension-set identity; it SHALL receive no direct database write access. The service SHALL validate shared-fact actions and apply reachability-dependent actions transactionally within the selected trail.

#### Scenario: Extension context records derived data
- **WHEN** a trusted custom expander performs a valid add-entry action through its context
- **THEN** the host journals the action and the service validates and writes it within its indexing transaction

#### Scenario: BDD target — Extension context records metadata patch
- **WHEN** executable behavior is covered by `features/extension-coordination/extension-coordination.feature::Extension context records metadata patch`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Expander attempts to bypass its context
- **WHEN** executable behavior is covered by `features/extension-coordination/extension-coordination.feature::Expander attempts to bypass its context`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Extension identity participates in trail selection
The service SHALL include the current trusted extension-set identity in trail compatibility and construct a new trail when it differs.

#### Scenario: Global expander changes
- **WHEN** an indexed repository observes a changed global expander source hash
- **THEN** its next synchronization rebuilds entries with the new extension behavior

### Requirement: Hook failures are diagnostics
Hook failures returned by the extension host SHALL be exposed as diagnostics without invalidating already valid canonical indexing.

#### Scenario: Hook failure with valid expansion
- **WHEN** expansion succeeds and an observing hook fails
- **THEN** the entry can commit and the hook failure remains visible to the client

### Requirement: Extension host execution is supervised
Every extension-host operation SHALL run under the configured positive deadline, defaulting to 30 seconds. The service SHALL close the request input, continuously collect output, and kill and reap the exact child when the operation times out or its process boundary fails. A fatal host failure SHALL return a structured service error and SHALL NOT publish a partial or replacement trail.

#### Scenario: Host exceeds its deadline
- **WHEN** an extension host invoked through a repository command remains running beyond the configured deadline
- **THEN** the command fails with a timeout diagnostic, the host is no longer running, and no partial or replacement trail is published

### Requirement: Host retries respect side-effect authority
A failed parser inspection or extension-identity operation SHALL be retried once through a fresh supervised process after the failed process is cleaned up. An expansion operation that can execute hooks SHALL NOT be retried automatically because hooks may perform external side effects.

#### Scenario: Parser inspection fails once
- **WHEN** a parser-only inspection fails on its first supervised host attempt and succeeds on its second
- **THEN** the repository command uses the second validated result and starts no third attempt

#### Scenario: Hook-bearing expansion fails
- **WHEN** an expansion that may execute hooks fails after the host starts
- **THEN** the repository command reports the failure without invoking that expansion again

### Requirement: Parser inspection supports typed batches
The service SHALL send parser-only inspection work as versioned batches whose items have stable request identities and SHALL accept only a response containing exactly one valid ordered result for every requested item. Missing, duplicate, reordered, extra, malformed, or incompatible results SHALL fail the batch without being used for attention selection.

#### Scenario: Compatible inspection batch
- **WHEN** selected history contains multiple uncached commit messages
- **THEN** the service obtains their annotation counts through a typed batch and associates every result with its requested commit in order

#### Scenario: Incomplete inspection batch
- **WHEN** a host omits one requested item from its batch response
- **THEN** the service rejects the entire response without selecting history from its partial counts
