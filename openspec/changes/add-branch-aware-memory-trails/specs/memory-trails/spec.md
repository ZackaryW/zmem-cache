## ADDED Requirements

### Requirement: Compatible projections are immutable trails
The service SHALL identify a trail by repository, resolved HEAD OID, effective attention identity, extension identity, and protocol/schema identity. A committed trail's identity, selected commit membership, and materialized outcomes SHALL be immutable; a changed input SHALL select or create another trail.

#### Scenario: Two branches share one trail
- **WHEN** two local branches resolve to the same OID under identical attention, extension, and compatibility identities
- **THEN** both selectors reuse the same immutable trail

### Requirement: Git references resolve live
Every ref-selected request SHALL resolve its Git commit-ish at request time and compare it with the client-observed OID before selecting a trail. Local branch aliases MAY cache the last resolved trail as a hint; tags, remote-tracking branches, detached OIDs, and other commit-ish selectors SHALL remain queryable without authoritative aliases.

#### Scenario: Cached branch alias is stale
- **WHEN** a local branch moved after its alias was recorded
- **THEN** the service ignores the stale alias, selects the trail for the live OID, and does not return the former branch state

#### Scenario: Ref moves after client observation
- **WHEN** native resolution differs from the OID observed by the requesting client
- **THEN** the request fails without creating or advancing a trail

### Requirement: Trails reuse repository-scoped immutable facts
Commits and reusable expansion facts SHALL be identified within a repository by commit OID and applicable parser/extension identity, and identical facts SHALL be shared by every compatible trail. Reachability membership and effective DECAY, CANCEL, META, conflict, and diagnostic state SHALL remain trail-specific.

#### Scenario: Cancellation exists on one branch
- **WHEN** two trails share a decision commit but only one contains a reachable CANCEL
- **THEN** both trails reuse the decision fact while only the CANCEL-containing trail reports it invalid

### Requirement: Trail construction is deterministic and atomic
The service SHALL select whole commits under the effective attention policy, order them deterministically parent-before-child, resolve effects within that selection, and commit membership and trail-specific state atomically. Failure SHALL expose neither a partial trail nor partial effects.

#### Scenario: META fails during trail construction
- **WHEN** a selected META range cannot be validated completely
- **THEN** no new trail or partial metadata overlay becomes visible

### Requirement: Existing anchors migrate to legacy trails without replay
Schema migration SHALL convert each existing repository anchor and materialized projection into an initial immutable legacy trail while preserving current scores, validity, relationships, and diagnostics. Existing entries SHALL have null/global affected areas, and migration SHALL NOT require Git-history replay. Missing reusable facts needed by another trail SHALL be recovered lazily from bounded selected history.

#### Scenario: Upgrade a populated cache
- **WHEN** a schema-three database containing an indexed repository is opened by the compatible new service
- **THEN** its current query state is available through a legacy trail without re-expanding historical commits

### Requirement: Query results identify trail provenance
The native query response SHALL report the requested selector, resolved HEAD, trail identity, selected commit usage, attention identity, extension identity, and protocol/schema identity together with entries, relationships, and diagnostics.

#### Scenario: Query a detached commit
- **WHEN** a client queries a detached commit OID
- **THEN** the response identifies the immutable trail for that OID without creating a local-branch alias
