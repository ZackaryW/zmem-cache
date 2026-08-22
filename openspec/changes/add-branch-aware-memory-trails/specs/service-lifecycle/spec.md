## MODIFIED Requirements

### Requirement: Queries observe the selected HEAD
Before answering a repository query, the service SHALL resolve the requested Git commit-ish or observed worktree HEAD, compare it with the client's observed OID, and select or construct a compatible immutable trail through that exact commit. Resolution, synchronization, or stale-ref failure SHALL return a structured error without returning another trail.

#### Scenario: BDD target — HEAD advanced before query
- **WHEN** executable behavior is covered by `features/service-lifecycle/service-lifecycle.feature::HEAD advanced before query`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Query a non-checked-out ref
- **WHEN** executable behavior is covered by `features/service-lifecycle/service-lifecycle.feature::Query a non-checked-out ref`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps
