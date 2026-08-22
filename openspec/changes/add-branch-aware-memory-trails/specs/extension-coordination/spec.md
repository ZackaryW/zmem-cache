## MODIFIED Requirements

### Requirement: Service validates extension-host exchanges
The service SHALL exchange protocol-v4 typed messages with the extension host and reject incompatible, malformed, or non-deterministically identified entry, relationship, DECAY, CANCEL, META, or diagnostic responses before committing shared facts or trail state.

#### Scenario: Protocol version mismatch
- **WHEN** the extension host reports a protocol version other than the compatible trail-and-META protocol
- **THEN** trail construction fails without publishing shared facts or trail state

### Requirement: Service remains the only canonical writer
The extension host SHALL return a typed journal of entry, relationship, decay, cancel, metadata-patch, and diagnostic actions performed through expander contexts, together with hook diagnostics and an extension-set identity; it SHALL receive no direct database write access. The service SHALL validate shared-fact actions and apply reachability-dependent actions transactionally within the selected trail.

#### Scenario: Extension context records metadata patch
- **WHEN** the built-in META expander records a valid ordered metadata-patch action
- **THEN** the service validates its endpoints, keys, types, operators, and complete selected range before atomically applying trail metadata

#### Scenario: Expander attempts to bypass its context
- **WHEN** an extension host response contains data not produced by a validated context action journal
- **THEN** the service rejects the response without publishing the candidate trail
