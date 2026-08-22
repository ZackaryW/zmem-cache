## MODIFIED Requirements

### Requirement: Capacity is configurable and rolling
The cache SHALL default to 3,000,000 stored entry facts and accept a positive `max_entries` override from `~/.zmem/config.toml`. After writes, it SHALL evict eligible unreferenced trail state and then eligible shared commit cohorts until capacity is satisfied or no eligible data remains. Shared facts referenced by any retained trail SHALL remain.

#### Scenario: Capacity exceeded by an unreferenced trail
- **WHEN** a write exceeds capacity and an old trail is no longer selected by a live ref or protected retention state
- **THEN** its membership and sparse state are evicted before shared facts still used by another trail

### Requirement: Eviction uses source committer time
Eviction ordering for trail state and shared commit cohorts SHALL use source Git committer time rather than database modification time. Equal timestamps SHALL use deterministic repository, trail, and commit identity ordering.

#### Scenario: Recently reused old fact
- **WHEN** an old shared commit fact is reused by a newly constructed trail
- **THEN** its source age remains unchanged and does not become newer merely because another trail referenced it

### Requirement: Recent commits are protected
Trail state and shared commit facts newer than the wall-clock cutoff of `protect_recent_days` SHALL be ineligible for eviction. The setting SHALL default to 14 days and `0` SHALL disable protection.

#### Scenario: Protected trail state exceeds capacity
- **WHEN** protected trail state alone exceeds `max_entries`
- **THEN** none is evicted and the cache temporarily exceeds the nominal capacity

### Requirement: Eviction preserves anchor correctness
Eviction SHALL NOT mutate a retained immutable trail, remove shared facts it references, move a live ref alias without fresh Git resolution, or cause already materialized trail effects to apply twice.

#### Scenario: Query after unreferenced trail eviction
- **WHEN** an old unreferenced trail is evicted while a newer trail retains overlapping commit facts
- **THEN** querying the newer trail reuses its existing state without replay caused solely by the eviction
