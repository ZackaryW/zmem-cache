Feature: Rolling cache retention
  Scenario: First start creates the per-user database
    Given an isolated user home without a zmem database
    When the store opens
    Then a usable entries database exists under .zmem/db

  Scenario: Capacity evicts the oldest eligible commit cohort
    Given stored commit cohorts exceed max_entries
    And the oldest cohort is eligible by committer time
    When retention runs
    Then every row owned by the oldest cohort is removed
    And database modification time does not affect its ordering

  Scenario: Recent protection overrides capacity
    Given protected commits alone exceed max_entries
    When retention runs with protect_recent_days set to 14
    Then no protected commit is evicted
    And the store reports that it remains over capacity

  Scenario: Zero disables recent protection
    Given old and recent commit cohorts exceed max_entries
    When retention runs with protect_recent_days set to 0
    Then cohorts are evicted by committer time until capacity is met

  Scenario: Eviction does not rewind an anchor
    Given entries behind a current repository anchor are evicted
    When that unchanged repository synchronizes again
    Then its anchored range is not replayed

  Scenario: Capacity exceeded by an unreferenced trail
    Given capacity is exceeded by old unreferenced trail state sharing commit facts
    When retention runs after a write
    Then unreferenced trail state is evicted before facts used by a retained trail

  Scenario: Recently reused old fact
    Given an old shared commit fact reused by a new trail
    When retention orders eligible shared facts
    Then reuse does not make the fact newer than its source commit time

  Scenario: Query after unreferenced trail eviction
    Given overlapping facts in a retained trail and an old unreferenced trail
    When the old trail is evicted and the retained trail is queried
    Then the retained state is reused without duplicate effect application
