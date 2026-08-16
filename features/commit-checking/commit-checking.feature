Feature: Simulate zmem commit effects without persistence
  Scenario: Fast check projects cancellation and rolls back
    Given an indexed repository with one valid decision
    When I fast-check a proposed cancellation of that decision
    Then the service reports the decision would become invalid with score 0.0
    And the stored entry and anchor remain unchanged after the check

  Scenario: Ordered effects share canonical evaluation
    Given an indexed repository with one valid decision
    When I fast-check a decay followed by cancellation of that decision
    Then the projected effects run in annotation order
    And the stored decision remains valid with score 1.0

  Scenario: Decay of an invalid decision is an explicit no-op
    Given an indexed repository with one cancelled decision
    When I fast-check a proposed decay of that decision
    Then the effect outcome is a no-op that does not restore the decision

  Scenario: Unresolved effect returns a rejected projection
    Given an indexed repository with one valid decision
    When I fast-check a cancellation of a missing target
    Then the check is unsuccessful with a rejected effect diagnostic
    And the stored decision remains valid with score 1.0

  Scenario: Deep check reconstructs a target outside persistent rows
    Given reachable history whose decision row is absent from the persistent cache
    When I deep-check a proposed cancellation of that decision
    Then isolated replay resolves and projects the cancellation
    And the absent decision is not copied into persistent rows

  Scenario: Check preserves extension trust and skips hooks
    Given a trusted repository with an active custom expander and hook
    When I fast-check its custom annotation
    Then the expander action and extension identity are reported
    And the hook does not run and is reported skipped

  Scenario: Deep proposed effect reports incomplete bounded history
    Given a proposed cancellation whose decision precedes its attention view
    When I deep-check it under that bounded attention policy
    Then the effect is unsuccessful because history is incomplete
    And persistent state remains unchanged

  Scenario: Unlimited deep attention reveals the proposed effect
    Given a proposed cancellation whose decision precedes its default attention view
    When I deep-check it with both attention limits unlimited
    Then complete replay reports cancellation from valid to invalid
    And persistent state remains unchanged
