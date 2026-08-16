# extension-coordination Specification

## Purpose

Defines how the Rust service obtains deterministic derived data from the trusted Python extension host without giving up database authority.

## Requirements

### Requirement: Service validates extension-host exchanges
The service SHALL exchange versioned typed messages with the extension host and reject incompatible, malformed, or non-deterministically identified responses before committing derived state.

#### Scenario: Protocol version mismatch
- **WHEN** the extension host reports an unsupported protocol version
- **THEN** repository indexing fails without advancing its anchor

### Requirement: Service remains the only canonical writer
The extension host SHALL return a typed journal of entry, relationship, decay, cancel, and diagnostic actions that expanders performed through their contexts, together with hook diagnostics and an extension-set identity; it SHALL receive no direct database write access. The service SHALL validate and apply journal actions transactionally.

#### Scenario: Extension context records derived data
- **WHEN** a trusted custom expander performs a valid add-entry action through its context
- **THEN** the host journals the action and the service validates and writes it within its indexing transaction

#### Scenario: Expander attempts to bypass its context
- **WHEN** an extension host response contains data not produced by a validated context action journal
- **THEN** the service rejects the response without advancing the repository anchor

### Requirement: Extension identity participates in anchoring
The service SHALL compare the current trusted extension-set identity with the repository anchor and rebuild the repository when they differ.

#### Scenario: Global expander changes
- **WHEN** an indexed repository observes a changed global expander source hash
- **THEN** its next synchronization rebuilds entries with the new extension behavior

### Requirement: Hook failures are diagnostics
Hook failures returned by the extension host SHALL be exposed as diagnostics without invalidating already valid canonical indexing.

#### Scenario: Hook failure with valid expansion
- **WHEN** expansion succeeds and an observing hook fails
- **THEN** the entry can commit and the hook failure remains visible to the client
