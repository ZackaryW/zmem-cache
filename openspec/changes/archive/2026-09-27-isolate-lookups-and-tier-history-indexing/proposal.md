# Proposal

## Why

Repository indexing currently occupies the service's serial request loop, delaying unrelated lookups and health checks until clients hit socket timeouts. Queries also reread Git history before discovering cached trails, and per-commit full ancestry enumeration makes nominally bounded work expensive.

## What Changes

- Reserve bounded worker and connection capacity for exact-trail lookups and independent health/control handling inside one per-user service.
- **BREAKING**: Cold queries enqueue deduplicated indexing and return structured `not_ready` within a short deadline instead of synchronously scanning history; no stale or partial trail is returned as success.
- Propagate deadlines through native transport, queueing, Git/host operations, and database work; isolate client disconnects and distinguish request cancellation from intentional background jobs.
- Schedule recent history (default 500 commits) ahead of resumable background prefetch through the newest 10,000 commits. Preserve query attention defaults of 500 commits/400 annotations and explicit overrides.
- Maintain one canonical SQLite WAL database, separate readers, and one publication writer; retain atomic immutable trails and correct cross-tier effects.
- Reuse exact cached trails before materializing history; batch Git reads and replace repeated full-ancestry enumeration with bounded graph work.
- Add admission bounds, fair scheduling, restart-safe checkpoints, job progress, and measurements for lookup latency and released resources.

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `service-lifecycle`: Concurrent request isolation, bounded exact-HEAD lookup outcomes, deadlines, and disconnect handling.
- `repository-indexing`: Prioritized resumable jobs, global host concurrency, deterministic publication, and bounded Git work.
- `cache-retention`: Concurrent snapshot readers, one writer, and bounded/pinned background staging.
- `commit-checking`: Isolate expensive checks from lookup capacity and cancel abandoned checks without weakening preview semantics.

## Impact

- Native code: `crates/zmem-svc/src/{main,lib}.rs`, `crates/zmem-core/src/lib.rs`, and `crates/zmem-store/src/lib.rs`; tests and corresponding behavior features.
- Protocol, configuration, schema migrations for job/checkpoint storage, runtime version compatibility, and README documentation.
- Coordinated Python client change in companion repository `../zmem`: preserve structured native errors and enforce an outer subprocess deadline. Companion planning is maintained separately because each OpenSpec root scopes implementation to its own repository.
- Existing single-database data remains migratable. No multi-database deployment, stale-success fallback, remote service, or weakening of attention/trail semantics.
