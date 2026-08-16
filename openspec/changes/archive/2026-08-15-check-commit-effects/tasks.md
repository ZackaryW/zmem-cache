## 1. Public Behavior Contract

- [x] 1.1 Shape independently runnable `features/commit-checking/` scenarios for fast effect projection, rollback, deep replay, historical evaluation, trust, and hook suppression.
- [x] 1.2 Run the new feature root fail-first and record the expected missing-command failures.

## 2. Canonical Evaluation Utilities

- [x] 2.1 Add store/core unit matrices for ordered effect outcomes, invalid/no-op targets, transaction rollback, and unchanged indexing semantics.
- [x] 2.2 Extract one canonical transactional action evaluator reused by committed range application and preview simulation.
- [x] 2.3 Add isolated replay utilities for proposed successors and existing refs using temporary storage and current extension identity.

## 3. Service Composition

- [x] 3.1 Extend the host request contract with explicit hook suppression and preview metadata.
- [x] 3.2 Add versioned daemon request/response fields and a native `zmem-svc check` command for fast and deep modes.
- [x] 3.3 Return structured actions, effect outcomes, diagnostics, extension identity, and hook state without persisting hypothetical state.

## 4. Verification and Reconciliation

- [x] 4.1 Run focused crate unit and `features/commit-checking/` GREEN verification.
- [x] 4.2 Run Cargo lock, supported toolchain, format, Clippy, complete workspace tests, independently runnable Behave roots, and release build gates.
- [x] 4.3 Reconcile the mature behavior into canonical specs and validate the OpenSpec change strictly.
