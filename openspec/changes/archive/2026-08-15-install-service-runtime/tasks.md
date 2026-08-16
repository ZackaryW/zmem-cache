## 1. Public Behavior and RED

- [x] 1.1 Extend the service-lifecycle feature with status identity, temporary-home isolation, persistent-host, and concurrent-start scenarios
- [x] 1.2 Prove the new public service-lifecycle scenarios fail before implementation

## 2. Service Utilities

- [x] 2.1 Add unit-tested runtime host resolution and typed service identity/status helpers
- [x] 2.2 Add unit-tested per-home startup locking with bounded stale-owner recovery

## 3. Service Wiring

- [x] 3.1 Expose structured non-starting status and release/protocol identity through `zmem-svc`
- [x] 3.2 Make installed binaries invoke the sibling persistent Python host while preserving explicit overrides and PATH development fallback
- [x] 3.3 Coordinate concurrent demand-start attempts and preserve alternate-home state isolation
- [x] 3.4 Make the service-lifecycle capability root GREEN through the built public binary

## 4. Verification and Specification

- [x] 4.1 Run Rust formatting, clippy, unit tests, the complete service-lifecycle Behave root, and a locked release build
- [x] 4.2 Strict-validate and reconcile the service-lifecycle OpenSpec delta
