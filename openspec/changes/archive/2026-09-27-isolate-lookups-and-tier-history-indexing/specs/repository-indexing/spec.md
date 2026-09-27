# Spec Delta

## MODIFIED Requirements

### Requirement: Indexing concurrency is globally bounded
The service SHALL bound simultaneous extension-host work across all indexing, identity, parser-inspection, and checking jobs to `max_concurrency`, defaulting to 8 and accepting a positive override from `~/.zmem/config.toml`. Validated results SHALL be applied in deterministic commit order. Speculative backfill SHALL NOT consume capacity reserved for requested work or lookup/control handling. Ready demand work SHALL take priority at batch boundaries while admitted background work receives bounded fair scheduling opportunities.

#### Scenario: More work than the configured bound
- **WHEN** indexing exposes more ready host work than `max_concurrency`
- **THEN** simultaneous host execution never exceeds the configured number and application order remains deterministic

#### Scenario: Concurrent repositories and checks
- **WHEN** multiple repositories and a deep check execute together
- **THEN** their combined live host count remains within the single configured global bound

## ADDED Requirements

### Requirement: History indexing uses resumable priority tiers
The service SHALL prioritize materialization of requested history, defaulting to the newest 500 commits subject to the existing 400-annotation attention bound, and subsequently prefetch older history in bounded lower-priority batches through a configurable default ceiling of 10000 newest reachable commits. Zero SHALL disable automatic prefetch. Prefetch SHALL NOT widen a query's attention policy or run side-effecting hooks. Explicit attention overrides SHALL remain supported, including unlimited demand work under resource/deadline controls. Background jobs SHALL be pinned to a resolved OID and checkpoint completed batches by immutable traversal identity so restart and HEAD movement cannot skip or double-count commits.

#### Scenario: Recent trail becomes usable before backfill
- **WHEN** a repository has 10000 reachable commits and no cached facts
- **THEN** its requested default trail can be published and queried before lower-priority older-history prefetch completes

#### Scenario: Background facts exceed query attention
- **WHEN** 10000 commits have been prefetched and a default query is issued
- **THEN** its selected trail still obeys 500 commits and 400 annotations with truthful truncation metadata

#### Scenario: Restart during a backfill batch
- **WHEN** the daemon restarts after some fact batches commit
- **THEN** the job reuses those batches and resumes its pinned traversal without publishing partial trails or automatically replaying uncertain hook execution

#### Scenario: HEAD moves during prefetch
- **WHEN** new commits or a history rewrite changes the live HEAD
- **THEN** a new lookup resolves the new OID, and old speculative work is coalesced or obsoleted without mixing its membership into the new trail

### Requirement: Tier boundaries preserve effect correctness
For each requested trail the service SHALL apply selected commits parent-before-child and atomically publish complete membership, entries, effects, metadata, and diagnostics. Reused or staged facts from another tier SHALL NOT independently determine final effect state. A target outside the selected attention view SHALL remain subject to existing incomplete-history rules even if its facts were prefetched.

#### Scenario: Recent cancellation targets older history
- **WHEN** a recent CANCEL targets a decision older than the default window and a later wider query selects both
- **THEN** the bounded trail remains unchanged and the wider trail resolves the cancellation exactly as one ordered replay would

#### Scenario: META crosses a tier boundary
- **WHEN** a META range crosses the recent/backfill boundary
- **THEN** metadata is published only under the existing complete-range rule and the result matches equivalent single-pass selected-history replay

### Requirement: Cached lookups avoid repeated history construction
An exact compatible published-trail lookup SHALL NOT reread selected commit messages or changed paths, invoke per-commit expansion, or enumerate selected history to reconstruct its cached membership. Indexing SHALL reuse compatible collected facts and SHALL NOT eagerly enumerate each selected commit's complete transitive ancestry. Work needed for graph correctness SHALL obey cancellation and attention incompleteness rules.

#### Scenario: Repeated warm query
- **WHEN** the same compatible HEAD and attention policy are queried again
- **THEN** lookup returns the published trail without per-commit Git reads or expansions

#### Scenario: Large linear history
- **WHEN** a 10000-commit linear history is indexed
- **THEN** graph storage and initial ancestry collection do not materialize the quadratic transitive ancestor-pair closure
