## MODIFIED Requirements

### Requirement: Capacity is configurable and rolling
The cache SHALL default to 3,000,000 stored entry facts and accept a positive `max_entries` override from `~/.zmem/config.toml`. After writes, it SHALL evict eligible unreferenced trail state and then eligible shared commit cohorts until capacity is satisfied or no eligible data remains. Shared facts referenced by any retained trail SHALL remain.

#### Scenario: BDD target — Capacity exceeded by an unreferenced trail
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Capacity exceeded by an unreferenced trail`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Eviction uses source committer time
Eviction ordering for trail state and shared commit cohorts SHALL use source Git committer time rather than database modification time. Equal timestamps SHALL use deterministic repository, trail, and commit identity ordering.

#### Scenario: BDD target — Recently reused old fact
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Recently reused old fact`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Recent commits are protected
Trail state and shared commit facts newer than the wall-clock cutoff of `protect_recent_days` SHALL be ineligible for eviction. The setting SHALL default to 14 days and `0` SHALL disable protection.

#### Scenario: BDD target — Protected trail state exceeds capacity
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Protected trail state exceeds capacity`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Eviction preserves anchor correctness
Eviction SHALL NOT mutate a retained immutable trail, remove shared facts it references, move a live ref alias without fresh Git resolution, or cause already materialized trail effects to apply twice.

#### Scenario: BDD target — Query after unreferenced trail eviction
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Query after unreferenced trail eviction`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps
