## Why

Repeated zmem queries should not rescan Git history or maintain independent process-local caches. A single cross-platform Rust service can own indexing and SQLite state, coordinate bounded concurrency, and serve consistent results to the new Python client.

## What Changes

- Introduce the always-on per-user `zmem-svc` service for Windows, macOS, and Linux.
- Add `zmem-svc add <path>` to register one Git repository, index its reachable history, and keep it synchronized.
- Store supported materialized entries in `~/.zmem/db/entries.db` while maintaining ancestry-aware repository anchors for incremental indexing.
- Apply `DECAY` and `CANCEL` transactionally as effects without storing them as entries or counting them toward capacity.
- Default to 50 global indexing workers and allow configuration through `~/.zmem/config.toml`.
- Default to a rolling capacity of 3,000,000 entries, evict whole oldest eligible commits by Git committer time, and protect entries from the most recent 14 days unless configured otherwise.
- Detect history rewrites and extension/schema changes and rebuild the affected repository instead of extending an invalid anchor.
- Invoke the coordinated Python extension host while retaining sole ownership of SQLite writes.

## Capabilities

### New Capabilities

- `service-lifecycle`: Per-user daemon lifecycle, repository registration, and local client communication.
- `repository-indexing`: Git traversal, supported-entry materialization, anchors, rebuilds, effects, and concurrency.
- `cache-retention`: SQLite ownership, configurable capacity, recent-entry protection, and deterministic rolling eviction.
- `extension-coordination`: Trusted Python extension-host invocation and extension-set invalidation across the service boundary.

### Modified Capabilities

None. This repository has no canonical specifications yet.

## Impact

- Establishes a Rust workspace containing reusable core/indexing logic, SQLite storage, and the `zmem-svc` binary.
- Creates the per-user database and configuration contract under `~/.zmem`.
- Coordinates with the `revamp-zmem-service` change in `zmem-2`, which owns public CLI behavior, annotation vocabulary, and Python extension definitions.
