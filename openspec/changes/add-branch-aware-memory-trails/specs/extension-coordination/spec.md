## MODIFIED Requirements

### Requirement: Service validates extension-host exchanges
The service SHALL exchange protocol-v4 typed messages with the extension host and reject incompatible, malformed, or non-deterministically identified entry, relationship, DECAY, CANCEL, META, or diagnostic responses before committing shared facts or trail state.

#### Scenario: BDD target — Protocol version mismatch
- **WHEN** executable behavior is covered by `features/extension-coordination/extension-coordination.feature::Protocol version mismatch`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Service remains the only canonical writer
The extension host SHALL return a typed journal of entry, relationship, decay, cancel, metadata-patch, and diagnostic actions performed through expander contexts, together with hook diagnostics and an extension-set identity; it SHALL receive no direct database write access. The service SHALL validate shared-fact actions and apply reachability-dependent actions transactionally within the selected trail.

#### Scenario: BDD target — Extension context records metadata patch
- **WHEN** executable behavior is covered by `features/extension-coordination/extension-coordination.feature::Extension context records metadata patch`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Expander attempts to bypass its context
- **WHEN** executable behavior is covered by `features/extension-coordination/extension-coordination.feature::Expander attempts to bypass its context`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps
