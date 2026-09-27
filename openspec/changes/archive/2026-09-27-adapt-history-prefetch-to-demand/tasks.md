# Tasks

## 1. Verify the prerequisite implementation

- [x] 1.1 Confirm `isolate-lookups-and-tier-history-indexing` tasks 1.3, 4.1, 5.3 and 6.3 are completed with recorded evidence for blocked-stdin/permit-wait cleanup, correct fast-check history budgets, failure retention beyond job capacity, and affected suite results; record the corrected baseline revision and binary identity and stop adaptive implementation here if those fixes remain open.

## 2. Demand policy and bounded access tracking

- [x] 2.1 Implement the selected-depth policy with 500-commit lead windows, two deeper observations at least 60 seconds apart, one-hour expiration and the existing speculative ceiling; verify clock-controlled tests for shallow-only traffic, 1000-to-1500 promotion, repeated-depth stability, annotation-limited selection, zero ceiling, end of history, decay and explicit unlimited-demand independence.
- [x] 2.2 Resolve stable repository/ref route identities alongside request resolution while keeping jobs pinned to exact OIDs and generations; verify branch advance, branch switch, ref aliases, detached OIDs and generation-change fixtures without additional history enumeration.
- [x] 2.3 Add transactional advisory-metadata migration, indexes and bounded store operations after the predecessor's final schema; verify populated-cache migration and rollback preserve exact trails, failures and checkpoints while creating no synthetic popularity, and record the native/client compatibility impact.
- [x] 2.4 Add bounded nonblocking observation recording and approximately five-second batched writer persistence, with 4096-route/8-MiB limits and startup expiration; verify a saturated-writer process test leaves cached lookups responsive and unit/store tests cover dropped events, crash loss, duplicate job completion, clock rollback and metadata limits.
- [x] 2.5 Document policy thresholds, advisory durability and observation exclusions in README, including the limit on distinguishing successful repeats from caller retries; verify the examples against the policy tests and effective existing configuration.

## 3. Demand-aware prefetch coordination

- [x] 3.1 Replace unconditional publication-triggered prefetch admission with qualified observations from successful jobs and published-trail hits; verify real-process scenarios for first/shallow lookups, shared query/add/fast-check completion, pending retries, control polling, failed work, and isolated deep checks.
- [x] 3.2 Pass justified targets and normalized routes to prefetch sessions, extending pinned coverage while reusing existing facts; verify 500-commit coverage increments execute in at most 64-commit turns, same-OID routes share work without shared popularity, and repeated 1000-commit queries stop at a justified 1500 target.
- [x] 3.3 Re-evaluate eligibility, ref generation, target and quota at batch boundaries and during restart recovery; verify stale eager checkpoints do not resume without demand, HEAD/ref rewrites do not mix membership, target shrink/expiry pauses work, and eligible work resumes surviving coverage correctly.
- [x] 3.4 Preserve foreground priority, rotating background turns and all service/host limits; verify mixed hot/cold repository demand with concurrent cached queries and control requests, then update lifecycle feature descriptions and README examples to match adaptive admission rather than automatic full backfill.

## 4. Safe speculative LRU reclamation

- [x] 4.1 Track actual demand recency and first reuse for speculative cohorts without copying shared facts; verify prefetch production/polling never refreshes recency, never-used data precedes reused data, shared-route reuse is accounted once, and eviction ties are deterministic.
- [x] 4.2 Integrate recency-ranked speculative reclamation with the canonical writer and existing staging quota; verify retained-trail, recent-history and active-job protections, shared facts, all-protected quota pause, and concurrent read-only snapshot correctness using store and real-process fixtures.
- [x] 4.3 Invalidate or repair checkpoints after reclamation and suppress immediate refill without new demand/capacity evidence; verify eviction/re-request, restart with missing facts and repeated quota-pressure scenarios make progress without skipping history or oscillating indefinitely.
- [x] 4.4 Preserve canonical source-time ordering and durable failure records independently of advisory eviction; verify the existing reused-old-fact scenario plus failed/uncertain-job queries after route demotion, metadata expiry and cache reclamation, and document the distinct retention rules in README.

## 5. Utilization and performance evidence

- [x] 5.1 Add bounded diagnostic accounting for unique prefetched facts/bytes, first demand reuse, unused eviction, remaining unused bytes, queue delay and skipped observations; verify known small fixtures prevent duplicate-use counts and confirm diagnostic collection does not put persistent writes on lookup response paths.
- [x] 5.2 Extend `benchmarks/tiered_history.py` with once-used repositories, repeated shallow views, progressive depth and mixed routes under quota pressure, including annotation-bearing merged histories; verify fixture assertions detect unwanted shallow backfill, missing deeper reuse and changed cross-window effect results, and document reproducible commands in `benchmarks/README.md`.
- [x] 5.3 Run matched corrected-predecessor/adaptive benchmarks at one and eight clients with repeated samples; deliver comparison of unused bytes, prefetch utilization, cold wider-demand latency and warm p50/p95/p99 with sample counts and variability, verify lower unused bytes in the mixed workload, and resolve or report a reproducible cached-p95 regression above 10 percent before accepting the change.

## 6. Integrated validation and rollout

- [x] 6.1 Run `cargo fmt --all -- --check`, workspace clippy with warnings denied, workspace tests and all affected native behavior features after integration; record results and resolve failures, including cross-window CANCEL/META, timeout cleanup and restart/reclamation interactions.
- [x] 6.2 Exercise matching companion client/runtime identity and deadline fixtures against the migrated backend, updating compatibility artifacts through the companion workflow if needed; verify typed outcomes and exact provenance, then rehearse stopped-daemon backup/restore or isolated rebuild with a compatible release pair.
- [x] 6.3 Validate this OpenSpec change strictly, verify every scenario has implementation/test evidence, and record predecessor-first spec synchronization and release sequencing in verification notes; keep implementation checkboxes open for any deferred requirement and distinguish planning readiness from runtime readiness.
