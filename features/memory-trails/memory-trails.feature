Feature: Immutable native memory trails
  Scenario: Two branches share one trail
    Given two local branches at one commit under identical trail identities
    When both branches are queried
    Then both selectors reuse one immutable trail

  Scenario: Cached branch alias is stale
    Given a cached local branch alias whose branch has moved
    When the branch is queried using its live commit identity
    Then the stale alias is ignored and the live trail is returned

  Scenario: Ref moves after client observation
    Given a ref that moves after the client observes its commit
    When the service resolves that ref for the request
    Then the request fails without publishing or advancing a trail

  Scenario: Cancellation exists on one branch
    Given two trails sharing a decision while only one reaches its cancellation
    When both trails are queried including invalid entries
    Then the shared decision is invalid only on the cancellation trail

  Scenario: META fails during trail construction
    Given a candidate trail containing an incomplete META range
    When that trail is constructed
    Then neither the candidate trail nor partial metadata becomes visible

  Scenario: Upgrade a populated cache
    Given a populated schema-three cache with a materialized projection
    When the compatible service opens the database
    Then a legacy trail preserves its query state without Git replay

  Scenario: Query a detached commit
    Given memory reachable from a detached commit identity
    When that commit is queried directly
    Then the response identifies its immutable trail without a local branch alias
