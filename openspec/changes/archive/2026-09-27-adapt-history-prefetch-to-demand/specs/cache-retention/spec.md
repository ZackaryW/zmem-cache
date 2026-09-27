# Spec Delta

## MODIFIED Requirements

### Requirement: Eviction uses source committer time
Eviction ordering for canonical trail state and shared published commit cohorts SHALL use source Git committer time rather than database modification or access time. Equal timestamps SHALL use deterministic repository, trail, and commit identity ordering. Recency-based reclamation SHALL apply only to eligible speculative cache data under its separate byte quota and SHALL NOT make an old canonical fact newer because it was accessed.

#### Scenario: BDD target — Recently reused old fact
- **WHEN** executable behavior is covered by `features/cache-retention/cache-retention.feature::Recently reused old fact`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: Popular old canonical fact
- **WHEN** frequent access refreshes advisory usage for an old fact referenced by a canonical trail
- **THEN** its source timestamp and canonical eviction ordering remain unchanged

## ADDED Requirements

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
