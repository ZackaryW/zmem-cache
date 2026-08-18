## 1. Public behavior contracts

- [x] 1.1 Extend the independently runnable extension-coordination feature with scenario-selected timeout cleanup, pure retry, no hook retry, and batch validation behavior
- [x] 1.2 Extend the independently runnable repository-indexing feature with scenario-selected inspection reuse and configured host-concurrency behavior

## 2. Host supervision utilities

- [x] 2.1 Add fail-first unit cases for timeout resolution, attempt policy, child cleanup, and default/invalid host configuration
- [x] 2.2 Implement the standard-library supervised host executor with closed input, concurrent output draining, deadline polling, kill-and-reap cleanup, and operation-specific attempts

## 3. Inspection batching and cache utilities

- [x] 3.1 Add fail-first protocol cases for ordered complete batch responses and missing, duplicate, reordered, extra, malformed, or incompatible items
- [x] 3.2 Add fail-first store cases for schema-2 migration and protocol-keyed inspection cache hit, miss, and invalidation behavior
- [x] 3.3 Implement protocol-3 batch types, additive schema-3 inspection storage, and transactional cache access

## 4. Service composition

- [ ] 4.1 Route identity, inspection, and expansion through the supervised executor with retry authority determined by operation
- [ ] 4.2 Batch and cache selected-history inspection while preserving attention results and deterministic commit application
- [x] 4.3 Change the default process concurrency to 8 and expose the positive 30-second host deadline configuration
- [ ] 4.4 Run the extension-coordination and repository-indexing feature roots independently and complete the Rust unit, lint, format, locked-build, and package-build gates

## 5. Documentation and release coordination

- [ ] 5.1 Update operational configuration and recovery documentation without asserting prose through tests
- [ ] 5.2 Coordinate protocol/schema identities and release ordering with the Python `zmem` change
