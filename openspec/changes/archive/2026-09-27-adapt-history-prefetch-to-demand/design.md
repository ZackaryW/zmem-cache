# Design

## Context

See proposal.md for motivation. The current working tree starts prefetch from every successful HEAD indexing job in `JobCoordinator::worker`. `PrefetchCoordinator` deduplicates `(repository, pinned HEAD)` and rotates work; `PrefetchSession::start` always chooses the configured ceiling. Sessions already process at most 64 commits per turn and checkpoint through the canonical writer. These are useful mechanisms to retain while replacing admission and target selection.

There is no access-history model today. Canonical retention deliberately uses Git committer time. Speculative storage has its own byte quota and checkpoint records, but reclamation currently uses checkpoint lifecycle/order rather than actual demand recency. The follow-up therefore needs advisory metadata and narrower speculative coverage without changing canonical timestamps or exact-trail semantics.

The predecessor `isolate-lookups-and-tier-history-indexing` remains at 23/27 tasks after its final assessment. Its input supervision, cancellation waits, fast-check budget and failed-job retention fixes must be verified first. This design consumes their corrected behavior; it does not hide those fixes inside an LRU implementation. The repository-indexing delta replaces a requirement introduced by that predecessor, so synchronization must occur in that order.

## Goals / Non-Goals

**Goals:** Bound speculative work by demonstrated depth; retain useful cached history; make tracking cheap enough for warm lookups; reuse existing worker, writer and checkpoint mechanisms; measure work saved and latency costs.

**Non-Goals:** Multiple databases; automatically unlimited scans; widening returned memory based on popularity; running hooks speculatively; replacing canonical retention with LRU; a new client polling protocol; learning a complex prediction model.

## Decisions

### 1. Use a small demand model with an explicit maximum lead

Use these initial policy constants, covered by a controllable clock in tests:

- One coverage window is 500 commits. Execution turns remain at most 64 commits and retain elapsed-time/byte limits.
- Credit at most one observation per route per 60 elapsed seconds. Keep observations for one hour, at most 60 per route. Expiration supplies decay without a frequency score that grows forever.
- Older-history promotion requires at least two credited observations with real selected history depth greater than 500 in that hour. Let `D` be the maximum depth among those unexpired older-history observations.
- For an eligible route, the maximum automatic target is `min(background_commit_limit, D + 500)`. No automatic work is needed if the ceiling is zero, the required facts already exist, or known reachable history ends before that target. Routes without sufficient deeper observations receive no speculative extension.
- Repeated 1000-commit requests can justify a target of 1500, but cannot justify 2000. A later qualifying request must actually use deeper history before the lead advances. Frequent 500-commit requests keep using the recent exact view without collecting older history.
- A request with a nominal limit of 10000 but only 120 selected commits due to node attention supplies depth 120. Fast check supplies the real-history depth after reserving its proposed-message nodes. Explicit unlimited demand remains subject to demand controls, not this target formula.

These thresholds are conservative starting choices, not measured optimum values. Keep them as documented policy constants initially; retain `background_commit_limit` as the user-facing speculative cap. A pure LRU does not represent depth, and lifetime LFU would keep old workloads permanently hot. Both are insufficient for admission on their own.

### 2. Count completed real demand and keep route identity separate from job identity

The route key is canonical repository plus normalized resolved ref identity. Symbolic HEAD maps to the current branch's full ref name, so ordinary branch advances keep access history while switching branches selects another route. Detached OIDs and expressions that cannot be tied unambiguously to a named ref use resolved-OID route keys. Gather this identity alongside existing ref resolution; do not enumerate history to classify a route.

Index jobs and cached trails still use exact observed OID, effective attention policy, trust/extension generation and version identity. A hot route never makes a different generation compatible. Store the current route generation with queued speculative work; after a ref update, invalidate the old automatic target and establish the new target only from a fresh demand resolution. Immutable matching facts remain reusable across generations.

An admitted real-history job contributes one observation when it succeeds, even when several query/add/check clients joined it. Its waiters and initial success handoff must not each add another observation. A direct published-trail hit can contribute a rate-limited observation. Job-status/health calls, `not_ready` responses, failed work, and rejected requests do not contribute. A request timeout is not an observation; a separately admitted service-owned job can still contribute once if its own work later succeeds. Deep replay does not create promotion demand for the persistent cache.

The current protocol cannot distinguish a successful manual repeat from a successful automated repeat indefinitely. Rate limiting and the fixed `D + 500` bound prevent a burst or retry loop from causing unlimited depth growth; do not claim perfect detection of caller intent. Pending retries are deduplicated by job identity.

### 3. Record advisory statistics without a lookup write dependency

Add a bounded in-memory accumulator and a small policy worker. Cap tracked routes at 4096 and total queued/accumulated advisory payload at 8 MiB; enforce both limits. Keep at most 60 interval observations per route and discard expired intervals. On pressure, discard old advisory records or skip observations. Missing evidence produces less prefetch, never broader work.

Flush coalesced observations approximately every five seconds through a nonblocking, low-priority submission to the existing canonical writer. Never hold a lookup read transaction, wait for writer admission, or wait for flush completion to return a query. Reuse the request's resolved identity and selected-depth metadata. Slow or failed advisory persistence is reported diagnostically and retried within bounded tracking capacity, without failing an otherwise valid lookup.

Persist bounded route observations and per-speculative-cohort last-demand/reuse accounting in the same SQLite database. The exact schema names can follow local conventions; required fields are route identity, observation timestamps/depths, latest resolved generation, cohort association, and accounting sufficient to distinguish produced from actually reused facts. Do not duplicate commit payloads or persist every request. Index route identity and eligible reclamation order.

Expire records on startup before queueing speculative work. Use monotonic time for live sampling; validate persisted wall-clock timestamps on load. Discard implausible future timestamps after clock rollback instead of retaining permanent heat. A crash may lose the last flush interval. This is acceptable because popularity is advisory; canonical data and durable failures retain their existing guarantees.

### 4. Admit only justified missing coverage and re-evaluate between batches

Replace the unconditional `prefetch.admit` after publication with a demand observation. The policy worker evaluates eligible routes and enqueues a target only after the requested view is usable. A 500-commit coverage extension can take several 64-commit execution turns. Reuse canonical and raw facts already available; a depth promotion must not reread every object from scratch.

Pass the justified target and resolved ref identity into `PrefetchSession`, instead of reading the global ceiling as the target. Support all normalized named refs consistently; the existing hard-coded live-HEAD obsolescence check must become a check of the job's resolved route. Detached work stays pinned and uses expiry/capacity rules. Different routes resolving to the same repository/OID share collection work at the maximum currently justified target, without transferring their popularity scores to each other.

At each bounded turn recheck target, ref generation, expiration and byte quota. A target increase extends the same pinned traversal using its validated coverage; a decrease or expiry pauses speculative collection at the next boundary. Existing collected facts need not be deleted immediately. Demand gets priority, while eligible background routes retain rotating bounded turns. Popularity never grants extra worker or host permits.

Restart consults both the persisted checkpoint and fresh eligibility. Reclamation can invalidate completed coverage, so a numeric completed count alone is insufficient after eviction: invalidate affected checkpoints or verify/recollect missing coverage before resuming. Keep enough immutable traversal/checkpoint identity to avoid skipping gaps. A route already at its justified target is not re-enqueued until actual demand, target, generation, or coverage changes.

### 5. Use LRU only among safely reclaimable speculative cohorts

Maintain a cohort's last actual demand-use time. Prefetch production, background scans, polling, and merely being popular do not refresh that time. Never-used cohorts have no demand timestamp and precede used cohorts; use oldest creation order and stable identities for ties. If compatible facts serve another route, record their actual demand reuse without copying the facts or promoting the other route's score.

Under the staging byte quota, choose least recently demanded eligible speculative cohorts. Recheck protections transactionally in the writer: retained-trail membership, canonical recent-history protection, and bounded active-job dependencies take precedence. Release completed/inactive speculative pins as allowed by their lifecycle; keep failed-job retry records separate from these cache ownership records. Removing one cohort does not delete a fact still protected elsewhere. If nothing can be reclaimed safely, pause speculation.

Track reclamation generation so a paused target does not immediately refill the same evicted data on every scheduler tick. Reconsider it after new qualified demand or a meaningful capacity change, with a bounded retry interval. An explicitly requested evicted view is ordinary demand and can recollect missing facts immediately under normal resource controls.

Canonical trail/cohort eviction continues to use Git committer time; the delta clarifies that speculative raw data has a separate policy. Replacing all retention with LRU was rejected because it would change established memory retention semantics and could damage failed-job safety.

### 6. Prove utilization and latency, not only cache hit counts

Record bounded aggregate counters for unique speculative facts/bytes collected, first subsequent demand reuse, unused eviction, remaining unused bytes, queue delay, and skipped advisory observations. Distinguish reuse from repeated reads of the same fact. Expose them to the benchmark through diagnostic snapshots or writer-persisted aggregate rows; no new public command or client retry behavior is required.

Extend `benchmarks/tiered_history.py` with workload patterns: many once-used repositories, repeated recent views, progressively deeper views, and mixed hot/cold routes under a tight staging quota. Include annotation-bearing histories and selected cross-window effects as well as the existing merge fixture. Use explicit simulated-clock policy tests for expiration; the end-to-end benchmark must preserve genuine process deadlines.

Compare a pinned corrected-predecessor binary with this implementation on identical fixtures, host mode, machine, trace setting and concurrency. Run one and eight lookup clients with repeated samples, report sample counts and variability, and retain the observed baseline even if it misses the existing 250 ms tuning target. Required behavioral outcomes are zero policy-induced older scans for once-used/shallow routes and actual reuse of prefetched facts when the deeper workload advances. Acceptance also requires lower unused prefetch bytes for the mixed workload and no unexplained material regression in cached lookup p95; investigate a reproducible increase above 10 percent rather than declaring it noise or adjusting the baseline after seeing results.

## Risks / Trade-offs

- Occasional wider requests stay cold longer -> explicit demand bypasses popularity and receives foreground priority; measure that latency separately.
- A highly active route may never request older history -> the selected-depth threshold prevents speculative full scans.
- Poll loops can resemble real successful demand -> rate-limit observations and cap lead relative to actual depth; document the protocol limitation.
- Advisory tracking can introduce new contention -> bounded nonblocking recording, batched low-priority writes, and a saturated-writer test.
- Promotion and eviction can oscillate -> generation-aware wakeups, decay, paused-target cooldown and no refill without a new reason.
- Alias sharing and reclamation can invalidate checkpoints -> preserve shared protections and explicitly test missing coverage on restart.
- Many policy settings increase maintenance cost -> start with documented constants, one existing depth cap, and a controllable test clock.

## Migration Plan

1. Complete and verify the predecessor's reopened tasks before implementation proceeds past this change's prerequisite gate. Record the corrected baseline revision and binary identity for comparative benchmarks. Synchronize predecessor specs before synchronizing this follow-up; do not archive or modify the predecessor merely to satisfy this change's planning status.
2. Add an additive, transactional migration after the predecessor's final schema version. Start routes with no demand observations; preserve trails, failures and compatible checkpoints. On first start, apply the new eligibility policy to recovered speculative jobs so old eager jobs do not automatically scan to 10000.

   User-approved compatibility policy: conflicting legacy derived tables may be removed transactionally and rebuilt for forward operation. Limit this fallback to identified incompatible legacy cache structures; do not delete compatible current trails or durable failed/uncertain job records, and do not silently open a database from a newer schema. This permission does not require dropping tables when an additive migration works.
3. Update native schema/release identity and corresponding companion compatibility fixtures through their proper repository workflow if the migration changes accepted versions. Keep protocol error meanings and explicit retry behavior unchanged.
4. Validate fixtures, targeted process tests, mixed-workload benchmarks, and the affected native/client suites. Publish measurements with limitations before enabling this policy in a release.
5. Roll back by stopping the daemon and restoring a compatible binary/database backup pair or rebuilding the derived cache in an isolated home. Setting `background_commit_limit = 0` disables speculation as an operational fallback; it is not a database-schema rollback.
