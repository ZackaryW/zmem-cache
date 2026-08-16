## Why

Effect validity depends on indexed repository state and therefore cannot be predicted faithfully by a standalone message-regex script or the Python parser alone. The sole-writer service needs a non-persistent check operation that reuses canonical journal validation and effect application semantics.

## What Changes

- Add a service check operation for a hypothetical successor message and an optional historical commit target.
- Synchronize the real repository before fast checking, then simulate journal actions and effects without advancing anchors or retaining hypothetical rows.
- Add isolated deep replay that builds the relevant reachable state outside the persistent cache before evaluating the selected message or commit.
- Return resolved action and effect outcomes, including before/after score and validity plus stable diagnostics.
- Run trusted expanders using the active extension set while instructing the host to skip hooks.
- Keep Git and persistent cache state unchanged by hypothetical evaluation; ordinary synchronization before a fast check remains allowed.

## Capabilities

### New Capabilities

- `commit-checking`: Defines service-owned fast simulation, isolated deep replay, canonical effect evaluation, structured results, and persistence boundaries.

### Modified Capabilities

None.

## Impact

- Affects `zmem-svc`, `zmem-store`, `zmem-core`, the Python host request contract, service protocol tests, and repository-indexing integration fixtures.
- Adds no database schema migration and no new third-party dependency.
- Coordinates with the Python `zmem check` command while leaving the `zpp` repository unchanged.
