# Final assessment, 2026-09-26

The changes make substantial improvements to scheduling, lookup cost, and failure handling. They are not cosmetic. However, the earlier 27/27 completion statement was too strong. This review reopened tasks 1.3, 4.1, 5.3, and 6.3; the native change is now 23/27 complete pending fixes and focused regression coverage.

## Improvements supported by implementation and evidence

- The committed service handled one connection at a time and ran synchronization inside query handling. The working tree has separate bounded lookup, heavy, and control queues, plus foreground indexing and background prefetch workers. A slow indexing request no longer has to block every cached query or health request.
- Cold queries admit durable jobs and return `not_ready`. Exact compatible published trails have a read-only lookup path that skips history materialization and per-commit expansion. Duplicate cold queries share work before a slow identity host finishes.
- Host permits are process-wide, with one slot reserved from heavy host work when capacity exceeds one. This is a real semaphore implemented with a mutex and condition variable. Job rows persist status; in-memory coordination dispatches work.
- Batched Git reads and reusable facts reduce repeated process launches. Parent-edge storage replaces eager transitive ancestry storage. The recorded 10,000-commit merged fixture stored 10,198 parent edges and zero closure rows.
- Background prefetch uses bounded 64-commit batches, checkpoints, a byte quota, and pinned commit identities. It collects raw history after requested publication; it does not fully expand every older annotation or widen a requested view.
- Canonical writes pass through one writer, and read-only WAL snapshots remain coherent during publication and retention. Typed errors distinguish pending work, overload, timeouts, and annotation results.

## Confirmed problems

### 1. A blocked host input write bypasses cancellation supervision

`crates/zmem-svc/src/lib.rs:1011` writes all request bytes to child stdin synchronously. The timeout/cancellation polling loop starts only after that write returns at line 1024. A host that does not read a sufficiently large request can retain its host permit and service worker past the request deadline.

A disposable real-process probe used a custom host that recorded its PID, did not read stdin, and slept four seconds. A deep check sent a 200,000-character proposed message with a 300 ms timeout. The native client returned typed timeout after 322 ms, but the request-owned host was still alive 1.1 seconds later. The fixture then allowed the host to exit and stopped its isolated daemon. This proves prompt client response does not yet guarantee prompt server resource cleanup.

Related inspection finding: `HostPermit::acquire` waits on its condition variable for the entire remaining deadline. A disconnect token does not notify that condition variable, so waiting requests can remain blocked until another host releases a permit. Add/fast-check admission also performs identity validation before installing the normal disconnect watcher.

### 2. Fast-check admission uses a wider history budget than the actual check

`admit_shared_history` in `crates/zmem-svc/src/main.rs:548` submits the original node limit. Later, `check_repository_with_attention` in `crates/zmem-svc/src/lib.rs:1922` subtracts the proposed message's nodes and synchronizes that narrower history again.

A cold repository with one historical decision was checked with one proposed decision and `--node-limit 1`. The proposed decision consumes the entire node budget. The check succeeded, but SQLite contained both a one-node historical trail and the expected zero-node history trail, plus one persistent expansion fact. Thus the shared job performed canonical expansion outside the history needed by this check. Indexing requests expansion with hooks enabled, so this can also execute unnecessary hook-bearing work. The existing shared-job test checks success and absence of a virtual commit, not absence of extra historical expansion.

### 3. Failed-job eviction can bypass explicit retry

Code inspection at `crates/zmem-svc/src/main.rs:1031` shows that reaching 256 tracked jobs permits eviction of a `failed` job. A later identical request then has no failure record to find and can create a new job. This conflicts with the promise that potentially side-effecting failed work requires explicit retry. This finding follows the admission code; a 256-job process-level reproduction was not run in this assessment.

## Limits of the performance claims

The recorded one-client warm p95 is 124.4 ms during a 10,000-commit merged/check/backfill run. An earlier eight-client burst measured 463.1 ms p95, above the 250 ms tuning target. These are synthetic Windows measurements with annotation-free history and only 30 warm samples, not a matched before/after comparison with the original timeout incident. They support responsive cached lookup under the tested load, not a universal speedup factor or proof that every `os error 10060` cause is eliminated.

Background repositories rotate between batches, but foreground indexing uses one FIFO worker that runs a whole requested job. Full fairness between competing long foreground jobs is not established. Waiting add/check requests also retain bounded connection threads; moving them outside heavy workers does not eliminate all waiting resources.

The existing native suite (84 scenarios), client suite (60 scenarios), and 123 client unit tests passed before this review. The targeted probes above expose gaps those tests did not cover. Supported Unix CI process-reap validation also remains outstanding.

## Recommendation

Keep the one-database design and the separate worker/permit architecture. Fix the confirmed cleanup, fast-check budget, and failed-job retention problems before describing the release as complete. No finding here establishes a need for multiple databases.
