## 1. Behavior contracts

- [x] 1.1 Add independently runnable Behave roots for service lifecycle, repository indexing, cache retention, and extension coordination with shared support and thin bindings
- [x] 1.2 Prove RED through the `zmem-svc` executable and coordinated extension-host boundary

## 2. Rust workspace and utilities

- [x] 2.1 Create workspace crates for core models/indexing, SQLite storage, and the service executable with a committed lockfile
- [x] 2.2 Add fail-first unit matrices for action-journal provenance and validation, ancestry decisions, configuration, protocol validation, and eviction selection
- [x] 2.3 Implement minimal Git command, configuration, identity, ordering, and typed-wire utilities and make their focused tests GREEN

## 3. Storage and indexing

- [x] 3.1 Implement versioned SQLite schema, repository registration, commit cohorts, entries, relationships, diagnostics, and anchors
- [x] 3.2 Implement fast-forward incremental indexing and repository rebuilds on rewrite, schema change, or extension-set change
- [x] 3.3 Implement atomic context-action and anchor transactions without persisting decay or cancel as entries or an effect ledger
- [x] 3.4 Implement globally bounded expansion work with deterministic application order
- [x] 3.5 Implement capacity enforcement, committer-time cohort eviction, recent protection, and soft-cap reporting

## 4. Service and coordination

- [x] 4.1 Implement `zmem-svc add <path>` with canonical validation, idempotency, and extension trust
- [x] 4.2 Implement per-user local service state, startup, request handling, synchronization, and query operations
- [x] 4.3 Implement bounded Python extension-host invocation and typed action-journal validation with extension identity rebuilds and hook diagnostics
- [x] 4.4 Make each capability-owned Behave root GREEN through `zmem-svc`

## 5. Verification and packaging

- [x] 5.1 Run rustfmt, Clippy with warnings denied, focused and complete Rust tests, and every capability Behave root independently
- [x] 5.2 Run a clean locked release build and coordinated end-to-end client/service scenario
- [x] 5.3 Document installation, configuration, service lifecycle, retention, trust, and recovery behavior
