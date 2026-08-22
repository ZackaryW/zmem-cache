Feature: Python extension-host coordination
  Scenario: Compatible context action is persisted by the service
    Given a compatible extension host journals a valid add-entry context action
    When the service validates its action journal
    Then the service can persist the entry without granting database access

  Scenario: Unjournaled output is rejected
    Given an extension host response containing data without valid journal provenance
    When the service validates its action journal
    Then the response is rejected and no anchor advances

  Scenario: Protocol mismatch leaves anchor unchanged
    Given an extension host with an unsupported protocol version
    When a repository range is indexed
    Then indexing fails and its anchor does not advance

  Scenario: Extension identity change requests rebuild
    Given an anchor containing the previous extension-set identity
    When the current extension host reports a different identity
    Then repository synchronization selects a rebuild

  Scenario: Hook diagnostics do not invalidate canonical expansion
    Given valid expansion output and a failing hook diagnostic
    When the service validates the response
    Then the entry remains valid for commit
    And the hook diagnostic remains visible

  Scenario: Timed-out host is reaped without advancing the anchor
    Given an indexed repository and an extension host that outlives its deadline
    When the next repository range is indexed
    Then a host-timeout error is returned
    And the timed-out host exits without advancing the anchor

  Scenario: Parser-only failure retries once
    Given a parser-only host that fails its first attempt and succeeds its second
    When repository attention is selected through the service
    Then selection succeeds after exactly two parser attempts

  Scenario: Hook-bearing expansion is not retried
    Given a hook-bearing expansion host that records attempts and fails
    When its repository range is indexed
    Then indexing fails after exactly one expansion attempt
    And the failed range does not advance its anchor

  Scenario: Incomplete inspection batch is rejected
    Given selected history whose inspection host omits one batch result
    When repository attention is selected through the service
    Then the incomplete batch is rejected before history selection

  Scenario: Compatible inspection batch preserves commit order
    Given selected history with multiple uncached commit messages
    When repository attention is selected through the service
    Then the inspection batch associates every result with its commit in order

  Scenario: Protocol version mismatch
    Given an extension host reporting an incompatible trail protocol
    When the service requests expansion for a candidate trail
    Then construction fails without publishing shared facts or trail state

  Scenario: Extension context records metadata patch
    Given a host journal containing a validated ordered metadata-patch action
    When the service constructs its selected trail
    Then the service validates and atomically applies the complete metadata range

  Scenario: Expander attempts to bypass its context
    Given a host response containing data absent from its validated action journal
    When the service validates the response
    Then it rejects the response without publishing the candidate trail
