# Spec Delta

This delta follows `isolate-lookups-and-tier-history-indexing`. Its modified requirement is introduced by that predecessor and must be synchronized first.

## MODIFIED Requirements

### Requirement: History indexing uses resumable priority tiers
The service SHALL prioritize materialization of requested history, defaulting to the newest 500 commits subject to the existing 400-annotation attention bound. Automatic older-history prefetch SHALL require recent repeated demand for older history and SHALL collect no more than one additional 500-commit window beyond the depth justified by that demand, capped by `background_commit_limit` (default 10000). Zero SHALL disable automatic prefetch. First or occasional use and repeated shallow use SHALL NOT independently trigger older-history prefetch. Prefetch SHALL NOT widen a query's attention policy or run expansion or side-effecting hooks. Explicit attention overrides SHALL remain supported, including unlimited demand work under resource/deadline controls, without requiring popularity or being limited by the speculative ceiling. Background jobs SHALL be pinned to a resolved OID and checkpoint completed batches by immutable traversal identity so restart and HEAD movement cannot skip or double-count commits.

#### Scenario: Recent trail becomes usable before backfill
- **WHEN** a repository has 10000 reachable commits, no cached facts, and receives its first default request
- **THEN** its requested trail becomes available without scheduling collection through 10000 commits solely because publication succeeded

#### Scenario: Background facts exceed query attention
- **WHEN** qualified older-history demand has caused 10000 commits to be prefetched and a default query is issued
- **THEN** its selected trail still obeys 500 commits and 400 annotations with truthful truncation metadata

#### Scenario: Restart during a backfill batch
- **WHEN** the daemon restarts after some qualified prefetch batches commit
- **THEN** it resumes only still-eligible pinned work using surviving coverage, without publishing partial trails or automatically replaying uncertain hook execution

#### Scenario: HEAD moves during prefetch
- **WHEN** new commits or a history rewrite changes the live HEAD
- **THEN** the next lookup resolves the new OID, route access history remains advisory, and speculative work is coalesced or obsoleted without mixing membership across OIDs

#### Scenario: Explicit demand exceeds the speculative ceiling
- **WHEN** an occasional user explicitly requests a view wider than `background_commit_limit`, including an unlimited view
- **THEN** the service admits or rejects that request according to demand capacity and deadlines, independently of popularity and the speculative ceiling

## ADDED Requirements

### Requirement: Prefetch promotion follows demonstrated history depth
The service SHALL distinguish frequent recent-history access from repeated use of older history. Promotion SHALL use the real selected history depth under the effective attention policy, including the history budget reserved for a fast check, and SHALL require qualified observations in separate observation intervals within a finite decay window. Repetition of the same view SHALL NOT increase speculative coverage beyond that view's justified depth plus one window. Expired observations SHALL stop justifying new speculative work, with demotion taking effect at a bounded batch boundary.

#### Scenario: Frequent shallow access
- **WHEN** a client repeatedly queries only the newest 500 commits
- **THEN** the service reuses compatible recent trails without promoting automatic collection to 10000 commits

#### Scenario: Repeated older-history use
- **WHEN** separate qualified observations demonstrate a selected depth of 1000 commits and the configured ceiling permits more
- **THEN** the service can prefetch through 1500 commits in bounded background batches

#### Scenario: Repeated access does not cause runaway promotion
- **WHEN** further requests keep selecting the same 1000-commit view
- **THEN** those requests do not advance the justified target beyond 1500 commits

#### Scenario: Annotation limits stop selection early
- **WHEN** a nominal 10000-commit request selects only 120 commits because of its annotation bound
- **THEN** its large nominal commit limit does not qualify it for older-history promotion

#### Scenario: Inactive demand expires
- **WHEN** a route has no qualifying demand remaining in the decay window
- **THEN** new speculative batches stop for that route while cached exact views remain usable subject to normal retention

### Requirement: Access observations preserve request responsiveness and identity
Access observations SHALL be advisory and bounded. Their recording SHALL NOT require a synchronous persistent write or waiting for indexing on the lookup path. The service SHALL group observations by canonical repository and resolved ref identity across ordinary HEAD advances, and SHALL keep detached commits and unrelated refs distinct. Coalesced requests for one pending indexing job SHALL count as one demand episode when its real history becomes usable. Job-status/control polling, pending retries, rejected requests, and failed or timed-out work SHALL NOT add promotion observations. Repeated successful requests SHALL be rate-limited per route so bursts cannot immediately establish sustained demand. Deep-check temporary replay SHALL NOT itself admit persistent prefetch work.

#### Scenario: Polling and pending retries
- **WHEN** clients repeatedly inspect a job or query the same not-yet-published view
- **THEN** polling adds no observations and the shared job contributes at most one demand episode on successful completion

#### Scenario: Successful request burst
- **WHEN** many successful requests for one route arrive in one observation interval
- **THEN** that interval contributes at most one promotion observation for the route

#### Scenario: HEAD advance and branch switch
- **WHEN** a branch advances and a client later switches to a different branch
- **THEN** access history can follow the advancing branch but does not confer its demand history on the unrelated branch

#### Scenario: Advisory recording is saturated
- **WHEN** advisory recording reaches its memory or writer-queue budget
- **THEN** the lookup still returns its exact result or ordinary typed failure within its deadline, and dropped observations cannot cause wider promotion

### Requirement: Demand and exact-view semantics take precedence over popularity
Popularity SHALL NOT change a requested trail's selected membership, extension validation, effect evaluation, failure/retry rules, or response status. Prefetch SHALL remain behind demand at bounded execution boundaries and within existing worker, host and byte limits. Eligible background routes SHALL receive bounded scheduling opportunities without a hot route monopolizing speculative capacity. A 500-commit coverage window SHALL be divisible into smaller execution batches.

#### Scenario: Cold demand arrives during hot-route prefetch
- **WHEN** an occasional repository needs its requested recent view while another route is prefetching older history
- **THEN** the demand receives foreground priority while lookup and control capacity remain available

#### Scenario: Cross-window effects
- **WHEN** a bounded query excludes the target of a CANCEL or the ancestry required by META even though that history was prefetched
- **THEN** it preserves the existing incomplete-history result, and only an explicit sufficiently wide view evaluates the complete effect

#### Scenario: Multiple eligible routes
- **WHEN** multiple routes have justified background work
- **THEN** they receive bounded turns without increasing the configured global resource limits
