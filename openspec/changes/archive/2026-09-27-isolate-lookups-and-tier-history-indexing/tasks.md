# Tasks

## 1. Protocol and deadline foundation

- [x] 1.1 Add typed native error codes/fields, JSON stderr failures, timeout parsing/defaults, and protocol identity changes; verify serialization/invalid-timeout tests and version-json compatibility fixtures.
- [x] 1.2 Propagate one monotonic budget through startup locks, ping, connect, socket framing and response writes; verify delayed-startup/slow-client tests do not multiply the deadline.
- [x] 1.3 Add cancellation-aware Git/host execution and database deadline/interrupt primitives; verify stalled children are killed/reaped and request-owned resources release within the controlled one-second cleanup target.
- [x] 1.4 Document native timeout defaults, error JSON, and retry semantics in README; verify examples against protocol fixtures and coordinate those fixtures with the companion client change.

## 2. Storage and efficient history facts

- [x] 2.1 Add transactional migration for parent edges, reusable raw/path facts, job/checkpoint state, and persisted lookup selection metadata; verify migration preserves existing immutable trails and rejects incompatible older writers.
- [x] 2.2 Separate one-time initialization, read-only snapshot connections, and the canonical writer; verify lookup reads remain coherent during publication and retention with concurrent-connection tests.
- [x] 2.3 Batch Git reads and cache changed paths; verify cold/warm Git invocation counts and existing root/rename/merge affected-area fixtures.
- [x] 2.4 Replace eager ancestry closure construction/consumption with cancellable parent-edge reachability and memoized range evaluation; verify CANCEL/DECAY/META and branch-merge fixtures plus linear graph growth on 10000 commits.
- [x] 2.5 Implement staging/unreferenced-prefetch byte accounting, bounded job pins and restart cleanup; verify quota pause, obsolete-job reclamation and retained-trail protection tests, and document quota/migration behavior.

## 3. Isolated request execution

- [x] 3.1 Replace the serial loop with bounded admission, reserved lookup workers and independent control capacity; verify cached queries/ping complete during blocked heavy work and incomplete client frames do not block acceptance.
- [x] 3.2 Hold exclusive daemon ownership for its lifetime and coordinate shutdown with workers/writer; verify concurrent startup after a delayed ping creates only one owner and shutdown leaves no active canonical writer.
- [x] 3.3 Keep disconnect/write failures local and cancel request-owned execution; verify a subsequent client can query the same daemon after another client times out or disconnects.
- [x] 3.4 Add validated capacity settings and queue/stage latency diagnostics; verify full queues return structured busy with bounded memory and document configuration defaults.

## 4. Priority scheduler and publication

- [x] 4.1 Implement durable job keys/states, coalescing before and after identity resolution, authenticated job-status and failure propagation; verify duplicate cold requests share work and failed jobs do not retry hooks automatically.
- [x] 4.2 Replace per-call host limits with a service-wide permit budget including identity and checks; verify combined execution never exceeds configurations of 1, 2, and 8 across repositories.
- [x] 4.3 Implement requested/recent priority, bounded speculative batches, round-robin fairness, promotion and HEAD-generation obsolescence; verify foreground progress under backfill, background progress under sustained demand, and bounded job backlog.
- [x] 4.4 Implement pinned-OID prefetch/checkpoints through the configurable 10000-commit ceiling without speculative expansion/hooks; verify restart resumes completed facts, HEAD rewrites do not mix membership, and zero disables prefetch.
- [x] 4.5 Assemble full requested projections from reusable facts and atomically publish through the writer; verify cross-tier CANCEL/META matches single ordered replay and injected failures expose no partial trail.
- [x] 4.6 Document job progress, background policy and retry/uncertain-hook behavior; verify README examples with isolated-home lifecycle/indexing feature scenarios.

## 5. Exact lookup and check integration

- [x] 5.1 Implement indexed candidate lookup before history materialization and conservative cached extension-identity validation; verify warm queries perform no per-commit Git/host work and source/trust/host/config changes invalidate candidates.
- [x] 5.2 Route cold queries to job admission and prompt not_ready, preserving stale_ref, timeout and busy distinctions; update lifecycle feature steps for explicit job completion/retry and verify exact-OID/attention provenance on success.
- [x] 5.3 Integrate add and fast/deep check with heavy scheduling, shared real-history jobs and cancellation; verify add retains its initial-index contract, preview remains non-persistent, and deep replay never substitutes persistent projections.
- [x] 5.4 Update README query/check flows and timeout/disconnect behavior; verify existing attention, metadata, trails and commit-checking suites retain their semantic results after asynchronous query setup.

## 6. Cross-system validation and rollout

- [x] 6.1 Exercise the matching companion client change `../zmem/openspec/changes/handle-bounded-service-lookups` with this native runtime in an isolated home; verify structured errors, outer deadlines, no automatic retry, and release protocol/schema selection agree.
- [x] 6.2 Benchmark cold/warm 10000-commit and merged histories with concurrent cached queries, deep checks and backfill; deliver measured p50/p95/p99 latency, Git counts, graph/staging growth, queue/host peaks, cancellation cleanup and publication/WAL costs, assessing the documented 250 ms warm-p95 tuning target.
- [x] 6.3 Run cargo fmt --check, cargo clippy --workspace --all-targets --locked -- -D warnings, cargo test --workspace --locked, and the affected behavior suites for lifecycle, indexing, checking, retention, extensions, metadata and trails; record results and resolve regressions.
- [x] 6.4 Validate compatible native/client version fixtures, transactional migration and stopped-service rollback/rebuild in disposable homes; verify the documented release sequence and no older writer accesses the new schema.
