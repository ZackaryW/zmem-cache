Feature: Layered commit metadata
  Scenario: Branch-local owner override
    Given two trails sharing an entry and only one containing a META owner assignment
    When both trails are queried
    Then only the META-containing trail reports the assigned owner

  Scenario: Rename crosses top-level areas
    Given a new commit renaming a file from a/old to b/sub/new
    When its shared commit fact enters the cache
    Then affected-area derivation includes a and b/sub

  Scenario: Four areas become global
    Given a new commit whose compact derivation has four areas
    When its shared commit fact enters the cache
    Then its affected areas are null and globally applicable

  Scenario: Filter a migrated entry
    Given a migrated legacy entry without an affected-area override
    When the trail is queried with any affected-area filter
    Then the entry reports null affected areas and remains visible

  Scenario: Invalid scalar append
    Given a trail with metadata targets in a complete range
    When META attempts to append to scalar owner
    Then no target metadata changes and the invalid operation is diagnosed

  Scenario: Range includes merged descendants
    Given a META range spanning commits on a merged branch
    When the selected trail applies the metadata patch
    Then every qualifying descendant and ancestor in the inclusive range is patched

  Scenario: Merge resolves concurrent metadata
    Given concurrent META assignments conflict before a merge
    When a descendant META assigns that key after the merge
    Then the descendant value is reported and the conflict is cleared
