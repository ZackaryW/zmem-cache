## Why

The native cache currently owns one mutable projection per repository, so switching branches rebuilds state and branch-specific effects cannot coexist. Monorepo affected-area filtering also needs cached, reusable Git-derived facts without backfilling every legacy commit during upgrade.

## What Changes

- Replace the single mutable repository projection with immutable TRAIL records keyed by repository, resolved HEAD, attention policy, extension identity, and protocol/schema identity.
- Resolve Git references live, retain non-authoritative local-branch aliases, and reuse commit facts across trails while storing DECAY, CANCEL, and META outcomes as sparse trail-specific state.
- Cache intrinsic affected areas only for commits newly entering the cache; migrate existing cached entries as null/global within an initial legacy trail without mandatory replay.
- Derive at most three compact affected areas from changed paths, including `<root>` and rename endpoints; classify broader changes as null/global.
- Apply protocol-v4 META patches atomically over complete reachable-ancestry ranges for the declared `affected_areas`, `owner`, and `tags` keys.
- Allow unreferenced trails to be evicted under cache capacity while retaining shared facts needed by another retained trail.
- **BREAKING**: advance protocol and database schema compatibility identities for the coordinated Python client.

## Capabilities

### New Capabilities
- `memory-trails`: Defines immutable cached trails, live reference resolution, shared facts, branch-specific state, legacy migration, and trail reuse.
- `commit-metadata`: Defines intrinsic affected areas, META overlays, declared keys, range behavior, and null/global compatibility.

### Modified Capabilities
- `repository-indexing`: Replaces the repository-wide anchor projection with trail selection and branch-aware reconciliation.
- `extension-coordination`: Adds the validated metadata-patch journal action and coordinated protocol identity.
- `cache-retention`: Applies bounded retention to trails and shared commit facts without breaking retained projections.
- `service-lifecycle`: Allows repository queries to select a resolved Git commit-ish rather than only the worktree HEAD.

## Impact

The Rust core Git model, service request protocol, SQLite schema and migration, projection reconciliation, effect application, retention, native tests, and capability-owned Behave roots change. The paired `zmem` release supplies the compatible host parser and public CLI surface.
