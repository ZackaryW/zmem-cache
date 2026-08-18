## Context

See `proposal.md`. `zmem-svc` currently calls a one-request Python host with three pipes for every identity, inspection, and expansion operation. `run_ordered` can start fifty such children, `wait_with_output` has no deadline, and attention selection reparses immutable commit messages on each command. The service is cross-platform, retains sole SQLite authority, and must preserve deterministic parent-before-child application and hook side-effect boundaries.

## Goals / Non-Goals

**Goals:**

- Bound peak child-process resources and guarantee cleanup on timeout and I/O failure.
- Avoid reparsing unchanged Git commits and batch remaining parser-only work.
- Preserve transactionality, deterministic selection/application, and one-request process isolation.
- Keep the implementation on the Rust standard library and existing dependencies.

**Non-Goals:**

- A persistent Python worker pool, cooperative cancellation inside extensions, or automatic retry of hook-bearing expansion.
- Changing attention limits, annotation grammar, extension trust, or hook authority.
- Coupling native and Python package release numbers.

## Decisions

### Supervise one-request children with standard-library primitives

Host execution will write and close stdin, drain stdout and stderr concurrently, poll `Child::try_wait` until a monotonic deadline, and on expiry call `kill`, `wait`, and join both drainers. A typed operation policy supplies the 30-second default deadline and attempt count. This keeps the current isolation boundary and avoids adding a process-management dependency; bare `wait_with_output` cannot enforce the required deadline, while a persistent pool would introduce module-state and recycling concerns beyond this change.

`Config` gains a positive `extension_host_timeout_seconds` value with default 30. `max_concurrency` defaults to 8 and remains a positive override. Parser inspection and identity use two total attempts. Expansion uses one attempt regardless of preview mode when hooks are enabled; no operation that may run hooks is retried.

### Batch only parser inspection

Protocol version 3 adds `inspect_batch`. Native requests contain ordered items with the commit OID as identity and message text; responses must repeat identities in the same order with typed counts and diagnostics. Batches are kept to a small fixed item count and scheduled under `max_concurrency`. Identity and expansion remain one request per child because they load extension state or may execute hooks.

### Cache inspection counts as additive derived state

SQLite schema version 3 adds a global inspection table keyed by commit OID and parser protocol identity. Commit OIDs make the message immutable, and protocol identity invalidates grammar changes. Selection reads exact matches, batches misses, validates the complete response, then stores the counts transactionally before applying attention selection. Proposed messages are never cached.

The version-2 database receives an additive migration that creates the table and advances `user_version`; existing repository rows remain derived evidence. Anchors carrying schema 2 will rebuild normally under schema 3, while a failed migration leaves the original transaction intact.

### Preserve ordered failure boundaries

Batch validation completes before any count is used. Host timeout, malformed output, retry exhaustion, or database failure returns the existing structured service error path. Repository projection and anchor updates remain after all required host results validate, so a fatal failure cannot publish a partial range.

## Utility Plan (Disposable)

Remove this section after the utilities below have independent fail-first proof and GREEN verification.

- `HostOperation::execution_policy(&Config) -> HostExecutionPolicy` maps identity and inspection to two total attempts, maps hook-capable expansion to one, and carries the positive `Duration` deadline.
- `execute_supervised(command: &mut Command, input: &[u8], deadline: Duration) -> anyhow::Result<Output>` closes stdin, drains both output streams concurrently, waits against a monotonic deadline, and kills plus reaps before returning a timeout or process error.
- `execute_host_output(config: &Config, operation: HostOperation, request: &Value) -> anyhow::Result<Vec<u8>>` owns bounded retry and validates process success before protocol decoding.
- `validate_host_inspection_batch(bytes: &[u8], expected_ids: &[String]) -> anyhow::Result<Vec<HostInspection>>` rejects every shape, identity, order, cardinality, and protocol mismatch as one batch failure.
- `Store::inspection(oid: &str, parser_protocol: u32) -> anyhow::Result<Option<InspectionRecord>>` and `Store::record_inspections(parser_protocol: u32, records: &[InspectionRecord]) -> anyhow::Result<()>` expose exact-key cache lookup and atomic insertion.
- `inspect_commits(config: &Config, store: &mut Store, commits: &[GitCommit]) -> anyhow::Result<Vec<HostInspection>>` preserves input order, reuses exact cache hits, batches misses, validates all misses, and persists them before returning.
- Schema opening owns `migrate_v2_to_v3(transaction) -> anyhow::Result<()>`; no other migration path is accepted.

## Risks / Trade-offs

- [Eight workers can reduce peak throughput on very large rebuilds] → Cached inspections and batching remove far more startup work than the reduced parallelism costs; users can raise the positive override.
- [Polling adds small scheduling latency] → Use a short bounded interval against a monotonic deadline and keep the worker count small.
- [Output drainer threads increase thread count] → The process bound caps them, and draining is required to prevent full pipes from deadlocking a child.
- [Schema migration introduces upgrade risk] → Keep it additive and transactional, and cover version-2 migration plus rollback in focused store tests.
- [A pure retry can repeat parser CPU work] → It cannot execute extensions or hooks and is capped at one retry.

## Migration Plan

Release protocol/schema version 3 in coordination with Python host support. On first database open, migrate schema 2 additively; rollback to the prior binary requires moving the derived database aside and rebuilding because the older binary rejects schema 3. Publish the native release before a Python release selects it as compatible.
