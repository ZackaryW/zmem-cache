## MODIFIED Requirements

### Requirement: Queries observe the selected HEAD
Before answering a repository query, the service SHALL resolve the requested Git commit-ish or observed worktree HEAD, compare it with the client's observed OID, and select or construct a compatible immutable trail through that exact commit. Resolution, synchronization, or stale-ref failure SHALL return a structured error without returning another trail.

#### Scenario: HEAD advanced before query
- **WHEN** the registered repository has a new commit and a client queries the newly observed HEAD
- **THEN** the service indexes or reuses facts through that commit and returns the immutable trail for the exact observed OID

#### Scenario: Query a non-checked-out ref
- **WHEN** a client queries a resolvable tag, remote-tracking branch, local branch, or commit OID that is not checked out
- **THEN** the service returns the compatible trail without modifying the worktree
