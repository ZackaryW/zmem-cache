# cache-retention Specification

## Purpose

Defines bounded SQLite persistence that evicts unreferenced trail state and shared facts predictably while protecting retained memory.

## Requirements

### Requirement: Cache uses the per-user database
The service SHALL be the sole writer of `~/.zmem/db/entries.db` and SHALL create its parent directories when necessary.

#### Scenario: First service start
- **WHEN** the database path does not exist
- **THEN** the service creates a usable schema before accepting repository work

### Requirement: Capacity is configurable and rolling
The cache SHALL default to 3,000,000 stored entry facts and accept a positive `max_entries` override from `~/.zmem/config.toml`. After writes, it SHALL evict eligible unreferenced trail state and then eligible shared commit cohorts until capacity is satisfied or no eligible data remains. Shared facts referenced by any retained trail SHALL remain.

#### Scenario: BDD target — Capacity exceeded by an unreferenced trail
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Capacity exceeded by an unreferenced trail`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Eviction uses source committer time
Eviction ordering for canonical trail state and shared published commit cohorts SHALL use source Git committer time rather than database modification or access time. Equal timestamps SHALL use deterministic repository, trail, and commit identity ordering. Recency-based reclamation SHALL apply only to eligible speculative cache data under its separate byte quota and SHALL NOT make an old canonical fact newer because it was accessed.

#### Scenario: BDD target — Recently reused old fact
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Recently reused old fact`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: Popular old canonical fact
- **WHEN** frequent access refreshes advisory usage for an old fact referenced by a canonical trail
- **THEN** its source timestamp and canonical eviction ordering remain unchanged

### Requirement: Recent commits are protected
Trail state and shared commit facts newer than the wall-clock cutoff of `protect_recent_days` SHALL be ineligible for eviction. The setting SHALL default to 14 days and `0` SHALL disable protection.

#### Scenario: BDD target — Protected trail state exceeds capacity
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Protected trail state exceeds capacity`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Eviction preserves trail correctness
Eviction SHALL NOT mutate a retained immutable trail, remove shared facts it references, move a live ref alias without fresh Git resolution, or cause already materialized trail effects to apply twice.

#### Scenario: BDD target — Query after unreferenced trail eviction
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Query after unreferenced trail eviction`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Concurrent lookup observes one complete published snapshot
The service SHALL retain one canonical per-user database and allow snapshot lookup while indexing prepares or publishes other trails. A response SHALL read one coherent published snapshot across summary, entries, relationships, and diagnostics even if retention runs concurrently. Unpublished staging SHALL NOT appear in query results. Index preparation and maintenance SHALL NOT require a lookup to wait for an entire history scan.

#### Scenario: Retention overlaps lookup
- **WHEN** retention removes an eligible trail while a lookup is reading its published snapshot
- **THEN** lookup either obtains the complete coherent snapshot or a structured unavailable outcome, never a mixture of missing and retained components

### Requirement: Background staging is bounded and reclaimable
The service SHALL enforce a finite configurable global staging-byte quota in addition to existing entry capacity, pause speculative prefetch under capacity pressure, and protect only bounded active-job dependencies until publication, failure, or obsolescence. Completed, failed, and obsolete jobs SHALL release their staging protection. Restart SHALL reconcile abandoned work without deleting facts needed by retained trails or representing incomplete work as published.

#### Scenario: Speculative work reaches staging capacity
- **WHEN** backfill reaches the configured staging-byte quota
- **THEN** it pauses without unbounded disk growth and existing published lookups remain usable

#### Scenario: Obsolete job releases pins
- **WHEN** a superseded speculative job is marked obsolete
- **THEN** its unreferenced staged data becomes eligible for reclamation while retained trail dependencies remain protected

### Requirement: Speculative reclamation uses demand recency safely
Under staging-byte pressure the service SHALL reclaim least recently demanded eligible speculative data before recently demanded eligible speculative data, using deterministic ties. Prefetch production and job polling SHALL NOT count as demand access. Reclamation SHALL preserve facts required by retained trails, existing protected data, and bounded active-job dependencies. Shared facts SHALL be reclaimed only after all protections are released. If no safe victim can free sufficient space, speculative work SHALL pause without evicting protected data or preventing an independent cached lookup.

#### Scenario: Cold and hot speculative data compete
- **WHEN** staging pressure requires reclamation and two unprotected speculative cohorts have different last-demand times
- **THEN** the less recently demanded cohort is reclaimed first

#### Scenario: Shared speculative fact is protected
- **WHEN** a cold cohort shares a fact with a retained trail or an active protected job
- **THEN** reclaiming the cold cohort does not delete that fact

#### Scenario: Only protected data remains
- **WHEN** staging remains full and all remaining facts are protected
- **THEN** speculative work pauses and a compatible published lookup remains available

#### Scenario: Reclaimed history is requested again
- **WHEN** a later request needs facts that speculative reclamation removed
- **THEN** the service recollects the missing facts under ordinary demand controls and does not treat an old checkpoint as proof that evicted coverage still exists

### Requirement: Access metadata is bounded and expendable
The service SHALL store advisory access metadata within the existing per-user database and enforce finite in-memory and persistent tracking limits. Old observations SHALL expire. Restart SHALL expire stale observations before admitting background work, and losing or discarding access metadata SHALL only reduce speculative promotion; it SHALL NOT invalidate a retained trail, lose a durable job failure, or widen a requested view. Migration SHALL preserve existing canonical trails, facts, job failures, and compatible checkpoints without treating pre-existing caches as evidence of popularity.

#### Scenario: Restart after inactivity
- **WHEN** the service restarts after the observation window has elapsed
- **THEN** stale popularity does not restart previously eligible speculative scans

#### Scenario: Metadata quota is reached
- **WHEN** many distinct refs exhaust the advisory tracking budget
- **THEN** tracking remains bounded and discarding an advisory record leaves canonical data and durable failure records intact

#### Scenario: Upgrade an existing cache
- **WHEN** the policy metadata migration opens a populated predecessor database
- **THEN** existing exact trails remain queryable, failed jobs remain failed, and routes start without synthetic access observations

### Requirement: Adaptive policy cannot authorize a failed job retry
Recency changes, promotion, demotion, reclamation and advisory metadata expiry SHALL NOT delete, reset or override the durable record that prevents implicit retry of failed or uncertain hook-bearing work. Only the established explicit retry operation or a separately valid new job identity SHALL authorize another attempt.

#### Scenario: Failed job is unpopular
- **WHEN** a failed job's route becomes inactive or its speculative facts and advisory records are evicted
- **THEN** an identical subsequent demand request still exposes the failure until an explicit retry is authorized
