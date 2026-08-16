Feature: Anchored repository indexing
  Scenario: Only supported entries consume capacity
    Given a commit with a supported entry, unsupported annotation, and valid effect
    When the commit is indexed
    Then one entry is stored and the effect is applied
    And the unsupported annotation is diagnosed

  Scenario: Fast-forward indexes only the new range
    Given a repository anchored at an ancestor of HEAD
    When two descendant commits are synchronized
    Then only those two commits are expanded before the anchor advances

  Scenario: Rewrite rebuild removes obsolete effects
    Given an indexed history that cancels a decision
    When HEAD is rewritten without the cancellation
    Then repository state is rebuilt and the decision is valid

  Scenario: Failed range is atomic
    Given indexing fails fatally after proposing an effect
    When the range transaction ends
    Then neither the effect nor the new anchor is visible

  Scenario: Expansion respects configured concurrency
    Given more ready commit work than max_concurrency
    When the repository is indexed
    Then simultaneous expansion never exceeds the configured bound
    And application remains deterministic

  Scenario: Unlimited attention replaces a bounded projection
    Given a repository anchored with an older decision outside its bounded view
    When the repository is queried with both attention limits unlimited
    Then complete history is rebuilt and the older decision is returned
    And the anchor reports a complete attention identity
