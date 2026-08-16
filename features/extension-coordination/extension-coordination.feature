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
