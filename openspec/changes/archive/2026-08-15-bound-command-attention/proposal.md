## Why

Native repository synchronization and deep checking currently materialize complete reachable ranges, so a large or adversarial history can consume unbounded work. The always-on service needs one deterministic attention policy shared by indexing, querying, and effect simulation while still allowing callers to request complete history explicitly.

## What Changes

- **BREAKING**: Bound repository attention by the most recent 500 commits and 400 syntactically valid zmem annotation occurrences by default.
- Accept resolved commit and node limits on repository service requests; each accepts a positive integer or `-1` for unlimited traversal.
- Resolve `ZMEM_COMMIT_LIMIT` and `ZMEM_NODE_LIMIT` above the built-in defaults while allowing explicit client flags to take precedence.
- Count entry, custom, unsupported, DECAY, and CANCEL annotations before expansion; exclude an entire boundary commit rather than partially evaluating it.
- Return structured attention metadata and preserve bounded-view identity so an anchor or cached projection is never represented as complete history accidentally.
- Replay bounded history in isolation for `zmem check --file <path> --deep`, count the proposed message's annotations against the node budget, and report an explicit incomplete-history outcome when a projected effect may depend on omitted history.
- Preserve `-1` as the explicit unlimited escape hatch and retain existing deterministic concurrency and database-retention limits as separate controls.

## Capabilities

### New Capabilities

- `command-attention`: Native commit/node attention budgets, environment/request resolution, selection semantics, and structured boundary reporting.

### Modified Capabilities

- `repository-indexing`: Initial indexing, rebuilds, and incremental synchronization operate on an explicitly bounded attention view.
- `commit-checking`: Deep proposed-message simulation replays the bounded view and distinguishes incomplete attention from conclusive effect rejection.

## Impact

The native request protocol, configuration/environment parsing, Git traversal, host response accounting, anchor/projection identity, store queries, service summaries, README, unit tests, and repository-indexing and commit-checking Behave roots are affected. Concurrency, rolling entry capacity, and recent-retention protection remain independent limits.
