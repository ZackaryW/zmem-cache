Feature: Versioned native service distribution
  Scenario: Assemble a complete typed release manifest
    Given staged service artifacts and identities for every supported target
    When I assemble the release manifest through its command
    Then the manifest describes the complete target set with verified sizes and checksums

  Scenario: Reject malformed identity metadata
    Given staged service artifacts with malformed identity metadata
    When I assemble the release manifest through its command
    Then release assembly fails without writing a manifest
