## MODIFIED Requirements

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

### Requirement: Check responses are structured and non-persistent
The service SHALL return a versioned structured result containing mode, repository, evaluated parent or target, extension identity, actions, effect outcomes, diagnostics, hook state, effective attention limits, observed usage, truncation state, and reached-bound reason. Fatal host, protocol, Git, storage, or attention-validation failures SHALL return a structured service error. Semantic results made inconclusive by omitted history SHALL remain available but unsuccessful. No check SHALL persist its virtual commit, projected actions, projected diagnostics, or projected anchor.

#### Scenario: Persistent state survives failed check
- **WHEN** action-journal validation or proposed-message attention validation fails during a check
- **THEN** the request fails and all previously stored entries, relationships, diagnostics, and anchors remain unchanged
