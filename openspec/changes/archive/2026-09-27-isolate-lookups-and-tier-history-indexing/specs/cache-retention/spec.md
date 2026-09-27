# Spec Delta

## ADDED Requirements

### Requirement: Concurrent lookup observes one complete published snapshot
The service SHALL retain one canonical per-user database and allow snapshot lookup while indexing prepares or publishes other trails. A response SHALL read one coherent published snapshot across summary, entries, relationships, and diagnostics even if retention runs concurrently. Unpublished staging SHALL NOT appear in query results. Index preparation and maintenance SHALL NOT require a lookup to wait for an entire history scan.

#### Scenario: Retention overlaps lookup
- **WHEN** retention removes an eligible trail while a lookup is reading its published snapshot
- **THEN** lookup either obtains the complete coherent snapshot or a structured unavailable outcome, never a mixture of missing and retained components

### Requirement: Background staging is bounded and reclaimable
The service SHALL enforce a finite configurable global staging-byte quota in addition to existing entry capacity, pause speculative prefetch under capacity pressure, and protect only bounded active-job dependencies until publication, failure, or obsolescence. Completed, failed, and obsolete jobs SHALL release their staging protection. Restart SHALL reconcile abandoned work without deleting facts needed by retained trails or representing incomplete work as published.

#### Scenario: Speculative work reaches staging capacity
- **WHEN** backfill reaches the configured staging-byte quota
- **THEN** it pauses without unbounded disk growth and existing published lookups remain usable

#### Scenario: Obsolete job releases pins
- **WHEN** a superseded speculative job is marked obsolete
- **THEN** its unreferenced staged data becomes eligible for reclamation while retained trail dependencies remain protected
