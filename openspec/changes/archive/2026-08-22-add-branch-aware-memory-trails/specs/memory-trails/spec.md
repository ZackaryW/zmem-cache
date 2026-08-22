## ADDED Requirements

### Requirement: Compatible projections are immutable trails
The service SHALL identify a trail by repository, resolved HEAD OID, effective attention identity, extension identity, and protocol/schema identity. A committed trail's identity, selected commit membership, and materialized outcomes SHALL be immutable; a changed input SHALL select or create another trail.

#### Scenario: BDD target — Two branches share one trail
- **WHEN** executable behavior is covered by `features/memory-trails/memory-trails.feature::Two branches share one trail`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Git references resolve live
Every ref-selected request SHALL resolve its Git commit-ish at request time and compare it with the client-observed OID before selecting a trail. Local branch aliases MAY cache the last resolved trail as a hint; tags, remote-tracking branches, detached OIDs, and other commit-ish selectors SHALL remain queryable without authoritative aliases.

#### Scenario: BDD target — Cached branch alias is stale
- **WHEN** executable behavior is covered by `features/memory-trails/memory-trails.feature::Cached branch alias is stale`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Ref moves after client observation
- **WHEN** executable behavior is covered by `features/memory-trails/memory-trails.feature::Ref moves after client observation`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Trails reuse repository-scoped immutable facts
Commits and reusable expansion facts SHALL be identified within a repository by commit OID and applicable parser/extension identity, and identical facts SHALL be shared by every compatible trail. Reachability membership and effective DECAY, CANCEL, META, conflict, and diagnostic state SHALL remain trail-specific.

#### Scenario: BDD target — Cancellation exists on one branch
- **WHEN** executable behavior is covered by `features/memory-trails/memory-trails.feature::Cancellation exists on one branch`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Trail construction is deterministic and atomic
The service SHALL select whole commits under the effective attention policy, order them deterministically parent-before-child, resolve effects within that selection, and commit membership and trail-specific state atomically. Failure SHALL expose neither a partial trail nor partial effects.

#### Scenario: BDD target — META fails during trail construction
- **WHEN** executable behavior is covered by `features/memory-trails/memory-trails.feature::META fails during trail construction`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Existing anchors migrate to legacy trails without replay
Schema migration SHALL convert each existing repository anchor and materialized projection into an initial immutable legacy trail while preserving current scores, validity, relationships, and diagnostics. Existing entries SHALL have null/global affected areas, and migration SHALL NOT require Git-history replay. Missing reusable facts needed by another trail SHALL be recovered lazily from bounded selected history.

#### Scenario: BDD target — Upgrade a populated cache
- **WHEN** executable behavior is covered by `features/memory-trails/memory-trails.feature::Upgrade a populated cache`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Query results identify trail provenance
The native query response SHALL report the requested selector, resolved HEAD, trail identity, selected commit usage, attention identity, extension identity, and protocol/schema identity together with entries, relationships, and diagnostics.

#### Scenario: BDD target — Query a detached commit
- **WHEN** executable behavior is covered by `features/memory-trails/memory-trails.feature::Query a detached commit`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps
