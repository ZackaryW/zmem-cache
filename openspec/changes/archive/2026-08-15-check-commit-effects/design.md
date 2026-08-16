## Context

See `proposal.md` and `specs/commit-checking/spec.md`. The daemon is the SQLite sole writer, validates Python action journals, resolves effects during ordered transactional range application, and may evict old entries without rewinding anchors. Fast preview must reflect that operational state; deep preview must reconstruct the semantic state independently.

## Goals / Non-Goals

**Goals:**

- Reuse canonical ordered action and effect rules for indexing and checking.
- Guarantee rollback of hypothetical state.
- Reconstruct full reachable state only when deep mode is requested.
- Preserve extension trust and deterministic current extension identity.

**Non-Goals:**

- Persisting preview history or adding schema tables.
- Running hooks or promising rollback of arbitrary trusted-expander side effects.
- Reproducing the historical extension source that existed at each old commit; rebuilds use the current active extension set.
- Modifying the separate ZPP repository.

## Decisions

### Add a versioned check request to the daemon and native CLI

The service request carries repository path, proposed message or existing ref, deep flag, and hook-execution false. The native `check` subcommand forwards to the daemon so Python and direct native callers share one owner. Mutually exclusive inputs are rejected before expansion.

### Extract transactional action evaluation from range persistence

Store action application will produce typed effect outcomes as it resolves targets and mutations. Normal range application commits and advances anchors; preview inserts a reserved all-zero virtual commit inside a transaction, captures actions, outcomes, and projected diagnostics, and explicitly rolls back. Both call the same evaluator.

Alternative: clone result rows and calculate changes in memory. Rejected because it duplicates SQLite target-selection and ordered mutation behavior.

### Fast check synchronizes then rolls back only the hypothetical successor

Fast mode performs the normal real-HEAD synchronization first, including legitimate cache writes and eviction. It then previews against the resulting persistent state. The response distinguishes synchronization from hypothetical evaluation.

### Deep check uses a temporary store and selected replay boundary

Deep mode creates an isolated database under an automatically managed temporary path, registers the repository with its existing trust decision, and replays parent-before-child commits. Proposed messages replay through `HEAD`; existing refs replay all reachable ancestors before expanding and evaluating the target once. The isolated store is disposed after the response.

Alternative: force a persistent rebuild. Rejected because it would destroy valid rolling-cache state and anchors merely to validate a message.

### Skip hooks at the host request boundary

Host expansion remains required for built-ins and trusted custom expanders. Both hook events are disabled for check requests and the response records that fact. This makes the public guarantee precise without attempting to sandbox trusted Python.

## Risks / Trade-offs

- [Process interruption can leave a temporary deep-check database] → Use unique temporary paths and best-effort scoped cleanup; never place them under the persistent database root.
- [Deep replay can consume substantial time and disk] → Require explicit `--deep`, reuse configured concurrency, and expose mode in the response.
- [All-zero virtual identity reaches custom expanders] → Add preview metadata to commit context and reserve the identity exclusively for non-persistent evaluation.
- [Refactoring effect application could change indexing] → Prove existing indexing matrices and new preview matrices against the same evaluator before wiring the command.

## Migration Plan

1. Add core/store evaluation types and fail-first unit coverage without changing the database schema.
2. Add daemon/native command wiring and public Behave coverage.
3. Release with the Python package version that invokes the new operation.
4. Rollback requires only reverting binaries; existing databases remain compatible.
