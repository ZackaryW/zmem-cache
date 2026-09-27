# Proposal

## Why

The tiered-indexing implementation schedules older-history prefetch after every successful HEAD index, even when a repository is visited once or clients only use its recent view. Using recent access and actual requested depth can reduce unused scans and storage while keeping useful older history available.

## What Changes

- Replace unconditional background collection with demand-aware admission: first and occasional requests materialize their requested view; repeated shallow access keeps the recent view useful without widening prefetch.
- Promote repositories with repeated use of older history to one additional 500-commit coverage window beyond recently demonstrated demand, capped by `background_commit_limit`. Repeated use of an unchanged shallow or deeper view cannot advance prefetch indefinitely.
- Keep exact request attention, explicit wider/unlimited requests, atomic trail publication, and service-owned jobs. Use the existing smaller execution batches and global permits; automatic collection remains raw facts without expansion or hooks.
- Track bounded, advisory access history by repository/ref across HEAD advances. Coalesce pending-job observations, exclude control polling, rate-limit repeated observations, and expire old demand.
- Reclaim eligible speculative data using access recency under the existing byte quota. Preserve canonical source-time eviction, retained-trail dependencies, and durable failed-job retry rules.
- Record prefetch utilization, unused bytes, and foreground latency so this policy can be compared with the previous eager policy.

## Capabilities

### New Capabilities

- None.

### Modified Capabilities

- `repository-indexing`: Demand-qualified prefetch depth, bounded promotion/demotion, stable route identity, and explicit-demand priority.
- `cache-retention`: Bounded advisory access metadata and recency-based reclamation of eligible speculative facts without changing canonical retention or failure records.

## Impact

- Native implementation: `crates/zmem-svc/src/main.rs` job admission and prefetch coordination; `crates/zmem-svc/src/lib.rs` lookup/synchronization results, prefetch sessions and writer mutations; `crates/zmem-store/src/lib.rs` advisory metadata, migration, checkpoints and speculative reclamation.
- Verification and guidance: native scheduler/store tests, lifecycle/indexing/retention behavior features, `benchmarks/tiered_history.py`, benchmark notes, and README configuration/lookup descriptions.
- Keep one SQLite database. A transactional schema migration must follow the predecessor's final schema version; coordinate native release identity and the companion client's existing runtime compatibility fixtures if that identity changes. No new client command or retry loop is planned.
- Dependency: implement and verify the remaining fixes in `isolate-lookups-and-tier-history-indexing` first, including supervised input/cancellation, correct fast-check history budgets, and durable failure retention. This follow-up does not close those tasks or claim the adaptive policy fixes them.
- Apply/sync this change after its predecessor. Its repository-indexing delta intentionally replaces the predecessor's unconditional prefetch requirement; the predecessor's artifacts stay intact as the record of that change.
