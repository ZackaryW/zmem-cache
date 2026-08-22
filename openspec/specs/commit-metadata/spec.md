# commit-metadata Specification

## Purpose

Defines typed intrinsic commit metadata and trail-specific META overlays.

## Requirements

### Requirement: Commit metadata is typed and layered
The service SHALL expose `affected_areas` as null or an ordered unique area array, `owner` as null or a string, and `tags` as an ordered unique string array. A trail's effective metadata SHALL layer its META overlay over the shared intrinsic metadata without modifying the underlying commit or entry fact.

#### Scenario: BDD target — Branch-local owner override
- **WHEN** executable behavior is covered by `features/commit-metadata/commit-metadata.feature::Branch-local owner override`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: New commit facts receive compact affected areas
When a commit first enters the shared cache after migration, the service SHALL derive intrinsic affected areas from its Git changes. Root-level files SHALL map to `<root>`; other paths SHALL be grouped by top-level directory and reduced to each group's deepest common parent; both rename endpoints SHALL participate; `<root>` SHALL count toward the maximum; and more than three resulting areas SHALL produce null/global.

#### Scenario: BDD target — Rename crosses top-level areas
- **WHEN** executable behavior is covered by `features/commit-metadata/commit-metadata.feature::Rename crosses top-level areas`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Four areas become global
- **WHEN** executable behavior is covered by `features/commit-metadata/commit-metadata.feature::Four areas become global`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Legacy affected areas remain global
Entries migrated into an initial legacy trail SHALL resolve missing affected-area metadata as null/global. A later META effect MAY narrow or reset that value for one selected trail without a migration-time path backfill.

#### Scenario: BDD target — Filter a migrated entry
- **WHEN** executable behavior is covered by `features/commit-metadata/commit-metadata.feature::Filter a migrated entry`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Metadata patches are validated and atomic
The service SHALL accept only ordered META operations for `affected_areas`, `owner`, and `tags`. Set SHALL replace a typed value, add SHALL insert a unique member only for a set-valued key, and null SHALL reset the key. Unknown keys, invalid types, canonical-field targets, or malformed operations SHALL reject the entire action without mutation.

#### Scenario: BDD target — Invalid scalar append
- **WHEN** executable behavior is covered by `features/commit-metadata/commit-metadata.feature::Invalid scalar append`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: META targets complete reachable ancestry
The service SHALL require unique endpoints in the selected trail, require `from` to be an ancestor of `to`, require both endpoints to precede the META commit, and target every selected commit that is both a descendant of `from` and an ancestor of `to`, inclusive. A missing or attention-truncated part of the range SHALL reject the complete patch atomically.

#### Scenario: BDD target — Range includes merged descendants
- **WHEN** executable behavior is covered by `features/commit-metadata/commit-metadata.feature::Range includes merged descendants`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

### Requirement: Metadata conflict resolution follows ancestry
A conflicting assignment SHALL replace an earlier assignment only when the new META commit descends from the earlier META commit. Incomparable conflicting assignments SHALL produce trail conflict state until a descendant META explicitly assigns a resolution.

#### Scenario: BDD target — Merge resolves concurrent metadata
- **WHEN** executable behavior is covered by `features/commit-metadata/commit-metadata.feature::Merge resolves concurrent metadata`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps
