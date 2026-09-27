# Design

## Context

See proposal.md for motivation. `serve` accepts and executes one request synchronously; `send_request` gives every response, including ping, a 120-second read timeout. Python invokes the native client without a subprocess timeout. `sync_repository_with_selection` obtains host identity and materializes history before checking for a retained trail. `GitRepo::commit` invokes Git for both message and paths; new projections enumerate full ancestry separately for every selected commit. `run_ordered` bounds each invocation, not concurrently executing jobs.

The store already uses WAL, immutable trail identities, reusable inspections/expansions, and temporary projections followed by atomic publication. Existing specs require exact observed HEAD, current extension identity, deterministic effects, and no partial publication. These remain constraints. The synchronous waiting contract in service-lifecycle and companion memory-cli changes explicitly.

## Goals / Non-Goals

**Goals:** Make lookup resource usage and latency independent of history reconstruction; prioritize useful recent history; preserve correct snapshots across merges, rewrites, cancellation, and retries.

**Non-Goals:** Multiple canonical databases, stale-success fallback, automatic unlimited history, speculative execution of side-effecting hooks, or replacing isolated deep-check semantics with cached answers.

## Decisions

### 1. Separate admission, lookup, control, and heavy work in one daemon

Use bounded queues and fixed worker capacity rather than unbounded thread-per-connection handling. The acceptor only performs bounded framing/authentication and dispatch; slow/incomplete clients have framing deadlines and size limits. Health/stop use independent capacity and do not acquire indexing or writer locks. Lookups use a reserved reader pool; add/check/indexing use separate heavy-work admission. Multiple daemons were rejected because ownership, startup, and publication would become harder without eliminating contention.

Initial configurable defaults are two lookup workers, a 64-request lookup queue, 32 admitted heavy jobs, and the existing eight total host permits. Reserve one heavy execution slot for requested/recent work; speculative backfill has at most one active batch globally and cannot consume all host permits when the configured limit exceeds one. With max_concurrency=1, alternate bounded demand/background turns using the same fairness rule instead of reserving an impossible second permit. These are conservative starting settings, not measured performance claims. Per-repository round-robin scheduling and one background turn after at most eight ready foreground batches prevent starvation. Waiting on a dependency releases execution/host permits: fast checks cannot hold every slot while waiting for indexing. A full queue returns `busy`, with retry guidance, immediately.

### 2. Exact-match lookup with one absolute deadline

Native query accepts `--timeout-ms` (positive; default 2000). Its monotonic deadline starts before service discovery/startup and covers lock acquisition, ping, connect, framing, queueing, ref resolution, compatibility validation, database reads, and output. Health operations default to 1000 ms; add/check default to 120000 ms and accept the same override. Startup attempts never reset these budgets. Startup ownership must not infer a dead daemon solely from an overloaded ping; retain a lifetime ownership lock before publishing service state.

Resolve the live ref and compare the observed OID. Locate candidate trails by repository, HEAD, attention policy, protocol/schema, and validated current extension identity, before loading commit messages or paths. Persist selection usage with the candidate so a cache hit need not recreate its bounded-view identity. Validate an extension dependency fingerprint covering configured host, trust, discovery inputs, and extension sources; reuse host-derived identity only while that fingerprint is verifiably current. If proving compatibility would exceed the deadline, return `not_ready` and validate in the indexing lane. Do not assume an mtime-only fingerprint proves compatibility or trust a stale branch alias.

An exact published trail returns immediately. A miss submits/coalesces a job and returns `not_ready` immediately after admission, rather than occupying a worker waiting for publication. `not_ready` includes requested OID, job ID/state, and retry-after milliseconds. If prerequisite identity resolution is pending, coalesce by canonical repository, observed OID, policy and configuration generation, then refine the job key after validation. A full queue returns `busy`; an expired operation returns `timeout`; ref mismatch remains `stale_ref`. Failed jobs expose a structured failure, not permanent `not_ready` or an automatic retry loop. A later query can observe completed work. No empty-success or older snapshot substitution.

Version the wire error object (`code`, `message`, `retryable`, optional `job_id`, `retry_after_ms`, `requested_oid`, `stage`) and preserve it through a single JSON stderr document on nonzero native exit. Bump protocol compatibility in both repositories. Job progress is exposed through an authenticated, deadline-bounded job-status request and native `job-status <id>` command.

### 3. Cancellation has explicit ownership

Carry remaining monotonic budgets and cancellation through all request-owned operations. Use supervised Git/host subprocesses, bounded socket reads/writes, database busy waits limited by remaining budget, and interruptible database reads. Deadline expiry signals cancellation and releases request-owned children/connections; target cleanup completion within one second in controlled tests. Check cancellation between bounded computation batches. A response write error only closes that connection, never exits the daemon.

Admitted indexing is service-owned: it has a durable job identity, independent batch budgets and admission limits, and may continue after its triggering lookup returns or disconnects. Deep checks are request-owned and stop when abandoned. Fast checks may leave a shared real-history indexing job, but their hypothetical work is cancelled. Never retry hook-bearing work automatically after uncertain execution. Shutdown stops admission, cancels request-owned work, checkpoints background jobs at a safe boundary and releases ownership only after writers have stopped.

### 4. Prioritize recent materialization, prefetch older facts incrementally

A first request pins the resolved OID. Materialize its requested attention window at high priority (defaults 500 commits/400 annotations); explicit wider requests also remain demand work and are divided into batches. Automatic backfill uses a separate configurable commit ceiling of 10000 (0 disables it), not a hidden widening of query attention. Schedule 64-commit speculative batches, bounded also by elapsed time and staged bytes. The recent requested trail is published before speculative older work starts. Resume any unprefetched portion of the first 500 and then positions 501 through 10000 in the same deterministic newest-first traversal.

Persist raw commit data, parent edges, changed-path metadata, parser inspections, and a checkpoint for that pinned traversal. Background prefetch does not run hooks or speculative expansion; demand expansion reuses compatible existing expansion facts and inspects prefetched data. This avoids surprising external effects and preserves hook execution authority. A later 10000-commit query may still need expansion/projection, but avoids recollecting prefetched history. It still obeys its explicit node bound.

Checkpoint by OID and traversal frontier/generation, not numeric offsets against moving HEAD. Restart resumes completed fact batches; interrupted hook-bearing expansion is marked failed/uncertain rather than replayed automatically. Deduplicate work within a repository and identity generation, promote relevant backfill to demand priority, and obsolete superseded speculative jobs so frequent HEAD advances cannot build an unlimited backlog. Explicit requests for older pinned OIDs remain valid. Validate trust and extension generations before publication; changed inputs produce new jobs/trails, never mixed results.

### 5. Preserve atomic trails and bound graph work

Tier outputs are facts, not independently final memory states. Construct each requested projection parent-before-child across its selected membership, then publish it atomically. A recent CANCEL targeting older prefetched history remains unresolved in a bounded trail that excludes that target; a wider requested trail resolves it after replay. Preserve META completeness rules and branch conflict behavior.

Use batched Git object reads and reuse cached changed paths. Persist parent edges, not the transitive ancestry closure; resolve reachability/ranges on demand with memoization and bounded/cancellable traversals. A 500-commit request must not enumerate all ancestors independently for every selected commit. If correctness needs history outside its attention view, retain existing incomplete-history semantics instead of silently extending attention. Replace ancestry closure consumers together and compare against the existing semantic fixtures.

### 6. One WAL database with a single publication writer

Keep `entries.db` canonical. Initialize/migrate once before admitting repository requests; open genuinely read-only reader connections without running migration/initialization on each lookup. Read entries, relationships, diagnostics, and summary in one short snapshot transaction so retention cannot tear a result. Do not retain a transaction while waiting for client socket delivery.

Heavy workers construct temporary projections and send bounded publication jobs to one writer. Recent publication has priority over speculative fact batches and maintenance. Stage facts/checkpoints separately from published trail membership; atomically publish complete membership and effects. Enforce a configurable global staging-byte quota (initial 256 MiB), including unreferenced prefetched facts after a batch/job completes, retain the existing entry quota, and pause speculative work at either pressure threshold. Active bounded jobs pin necessary staging; completion/failure/obsolescence releases pins; restart reconciles orphaned pins and staging. Explicit demand work that cannot fit its staging budget returns a structured capacity failure rather than waiting indefinitely or bypassing the quota. Budget cleanup and WAL checkpoint work so maintenance does not monopolize service execution. Per-repository databases are deferred until measurements demonstrate remaining write isolation needs.

## Risks / Trade-offs

- Cold queries now require a later explicit retry -> stable `not_ready`/job progress and matching Python client release; no hidden polling.
- Fast identity validation is subtle -> cache only proven-compatible dependencies and invalidate conservatively; test trust/config/source changes.
- Read slots cannot reserve disk bandwidth or CPU -> globally throttle heavy work and measure lookup tails under load.
- Arbitrary extension side effects cannot be rolled back -> no speculative hooks or automatic replay of uncertain hook work.
- Large publication can delay other writes -> stage immutable facts in bounded batches, prioritize publication, keep snapshot reads independent, and instrument transaction duration.
- Backfill creates non-entry data not counted by `max_entries` -> explicit staging quota and cleanup, with background pause under pressure.

## Pre-identity jobs and shared-history waiters

The dispatcher reserves a durable job before potentially slow identity validation and lets `add` and fast `check` wait for real-history publication outside the heavy execution pool:

1. Reserve a durable pre-identity job by canonical repository, observed OID, attention policy, trust/configuration generation before potentially slow host identity validation. Refine that reservation to the validated extension generation without changing its public job ID. A duplicate cold query joins the reservation immediately. Identity failure records a failed job, and a changed generation creates a distinct job; uncertain hook-bearing work is never retried automatically.
2. Let `add` and fast `check` attach a bounded waiter to the shared real-history job. Park the request outside the heavy execution pool while it waits, preserving its original deadline and disconnect cancellation. On job readiness, re-admit only the request-owned finalization or hypothetical preview; on timeout or disconnect, remove that waiter while the service-owned indexing job continues. `add` still returns only after its initial index is usable; deep `check` keeps its isolated replay and never uses a persistent trail as its answer.
3. Bound waiter count and resumption admission, expose `busy` when full, and cover duplicate cold requests, failed identity, add/check waits, disconnect, timeout, and continued cached lookup/control availability with real-process tests.

The waiter is bounded by heavy admission and the original request deadline. Its disconnect cancels only the request-owned wait; the service-owned job continues. A ready job hands its indexed-commit count to the first `add` response, while fast-check hypothetical state remains uncommitted. Deep checks keep isolated replay. This preserves the one-database and exact-HEAD decisions.

## Migration Plan

1. Implement native protocol/schema changes with transactional migration retaining existing trails; discard rebuildable ancestry-closure data only after parent-edge/reachability consumers are ready.
2. Complete companion `../zmem/openspec/changes/handle-bounded-service-lookups` before enabling the new installed runtime. Publish compatible native release before the Python release selects it; mismatched runtimes fail clearly rather than interpreting error strings.
3. Validate cold/warm, multi-repository, restart, and disconnect behavior in isolated homes. Benchmark a synthetic 10000-commit linear history and merged histories; record p50/p95/p99 lookup latency, Git invocations, ancestry/storage growth, host count, queue wait, publication time, cancellation cleanup, and WAL size. Initial warm p95 target is 250 ms under backfill on the recorded reference machine; correctness and configured deadline enforcement are mandatory independent of that tuning target.
4. Roll back by stopping the daemon, restoring a compatible binary/client pair, and restoring a pre-upgrade cache backup or rebuilding the derived cache in an isolated/new home. Never run an older writer against the migrated schema.
