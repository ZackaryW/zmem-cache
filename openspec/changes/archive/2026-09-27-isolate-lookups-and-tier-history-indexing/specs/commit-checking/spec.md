# Spec Delta

## ADDED Requirements

### Requirement: Checks are isolated and deadline bounded
Fast and deep checks SHALL use heavy-work capacity without consuming reserved lookup/control workers and SHALL share the global host bound. Their request deadline SHALL include queueing, real-history synchronization where applicable, replay, and proposed-message evaluation. Expiry or disconnect SHALL cancel request-owned replay and hypothetical work, reclaim temporary storage and owned subprocesses, and return a service timeout rather than an annotation-invalid result. Independently admitted shared real-history indexing SHALL remain service-owned. Existing deep-check isolation, trust, skipped-hook, and hypothetical non-persistence requirements SHALL remain unchanged.

#### Scenario: Deep check overlaps a cached query
- **WHEN** a deep check replays a large selected history
- **THEN** another client can query a published compatible trail within its lookup deadline

#### Scenario: Abandoned deep check
- **WHEN** the check client disconnects or its budget expires
- **THEN** replay stops, temporary/request-owned resources are reclaimed, and hypothetical results are never published
