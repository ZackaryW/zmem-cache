Feature: Bound native repository attention
  Scenario: Native defaults are reported
    Given a repository with one supported annotation
    When it is queried with no attention override
    Then native attention reports commit limit 500 and node limit 400
    And the complete view reports one selected commit and one annotation

  Scenario: Effects and unsupported annotations consume node attention
    Given recent history whose next whole commit would exceed node attention
    When it is synchronized with unlimited commits and node limit 2
    Then the boundary commit is excluded in full
    And effects and unsupported annotations count toward the reached node bound

  Scenario: Explicit request limits override environment independently
    Given environmental commit and node limits of one
    When a query explicitly requests commit limit 3 and node limit 2
    Then native attention reports commit limit 3 and node limit 2

  Scenario: Invalid attention is rejected before traversal
    Given a repository with one supported annotation
    When it is queried with node limit zero
    Then a structured request failure identifies node limit
