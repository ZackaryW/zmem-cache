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

  Scenario: Default host concurrency is resource-conservative
    Given more ready host work than the default concurrency
    When the repository is indexed without a concurrency override
    Then simultaneous host execution never exceeds eight
    And the service reports max_concurrency eight

  Scenario: Repeated selection reuses immutable inspections
    Given stable history observed through a counting inspection host
    When the repository is queried twice without parser or history changes
    Then the second selection starts no inspection hosts
    And both attention results are identical

  Scenario: Parser identity change invalidates inspections
    Given history inspected under a previous parser identity
    When the repository is queried under the current parser identity
    Then every stale inspection is replaced before attention selection

  Scenario: Unlimited attention replaces a bounded projection
    Given a repository anchored with an older decision outside its bounded view
    When the repository is queried with both attention limits unlimited
    Then complete history is rebuilt and the older decision is returned
    And the anchor reports a complete attention identity

  Scenario: Mixed supported and unsupported annotations
    Given a selected commit with a supported entry, unsupported annotation, and valid effect
    When its immutable trail is indexed
    Then all annotations consume attention, only the entry consumes capacity, and the effect updates trail state

  Scenario: Fast-forward branch update
    Given a retained trail whose branch advances by two commits
    When the advanced branch is queried under the same compatible view
    Then a distinct trail reuses prior shared facts and indexes the new commits

  Scenario: History rewrite removes a cancellation
    Given a retained trail reaching a cancellation and a rewritten branch without it
    When the rewritten branch is queried
    Then its new trail reports the uncancelled state while the former trail stays immutable

  Scenario: Unlimited request follows a bounded trail
    Given only a default-bounded trail for a repository
    When the repository is queried with unlimited commit and node attention
    Then a complete-history trail is constructed instead of reusing the bounded view as complete

  Scenario: Indexing fails while applying an effect
    Given a candidate trail with a fatal effect failure
    When indexing reaches that effect
    Then neither partial target state nor a partial trail is visible
