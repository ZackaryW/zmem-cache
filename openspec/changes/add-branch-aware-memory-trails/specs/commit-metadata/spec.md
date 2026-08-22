## ADDED Requirements

### Requirement: Commit metadata is typed and layered
The service SHALL expose `affected_areas` as null or an ordered unique area array, `owner` as null or a string, and `tags` as an ordered unique string array. A trail's effective metadata SHALL layer its META overlay over the shared intrinsic metadata without modifying the underlying commit or entry fact.

#### Scenario: Branch-local owner override
- **WHEN** a trail contains a valid META owner assignment absent from another trail sharing the target commit
- **THEN** only the META-containing trail reports the assigned owner

### Requirement: New commit facts receive compact affected areas
When a commit first enters the shared cache after migration, the service SHALL derive intrinsic affected areas from its Git changes. Root-level files SHALL map to `<root>`; other paths SHALL be grouped by top-level directory and reduced to each group's deepest common parent; both rename endpoints SHALL participate; `<root>` SHALL count toward the maximum; and more than three resulting areas SHALL produce null/global.

#### Scenario: Rename crosses top-level areas
- **WHEN** a newly cached commit renames a file from `a/old` to `b/sub/new`
- **THEN** both `a` and `b/sub` participate in compact affected-area derivation

#### Scenario: Four areas become global
- **WHEN** compact derivation produces four distinct areas
- **THEN** the shared affected areas are null/global

### Requirement: Legacy affected areas remain global
Entries migrated into an initial legacy trail SHALL resolve missing affected-area metadata as null/global. A later META effect MAY narrow or reset that value for one selected trail without a migration-time path backfill.

#### Scenario: Filter a migrated entry
- **WHEN** a legacy entry has received no META affected-area override
- **THEN** it matches every affected-area query

### Requirement: Metadata patches are validated and atomic
The service SHALL accept only ordered META operations for `affected_areas`, `owner`, and `tags`. Set SHALL replace a typed value, add SHALL insert a unique member only for a set-valued key, and null SHALL reset the key. Unknown keys, invalid types, canonical-field targets, or malformed operations SHALL reject the entire action without mutation.

#### Scenario: Invalid scalar append
- **WHEN** META attempts `owner+=platform`
- **THEN** no target metadata changes and a diagnostic identifies the invalid operation

### Requirement: META targets complete reachable ancestry
The service SHALL require unique endpoints in the selected trail, require `from` to be an ancestor of `to`, require both endpoints to precede the META commit, and target every selected commit that is both a descendant of `from` and an ancestor of `to`, inclusive. A missing or attention-truncated part of the range SHALL reject the complete patch atomically.

#### Scenario: Range includes merged descendants
- **WHEN** a merged branch contains commits descending from `from` and leading to `to`
- **THEN** the complete qualifying ancestry receives the patch

### Requirement: Metadata conflict resolution follows ancestry
A conflicting assignment SHALL replace an earlier assignment only when the new META commit descends from the earlier META commit. Incomparable conflicting assignments SHALL produce trail conflict state until a descendant META explicitly assigns a resolution.

#### Scenario: Merge resolves concurrent metadata
- **WHEN** a descendant META after merge assigns a value for a key conflicted by two branches
- **THEN** the selected trail reports the descendant value and clears that key's conflict
