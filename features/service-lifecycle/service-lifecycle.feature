Feature: Per-user zmem service lifecycle
  Scenario: Local client starts a stopped service
    Given no zmem service is running for the isolated user home
    When an authorized local client ensures the service
    Then it can connect to one per-user service

  Scenario: Add registers and indexes one repository
    Given a Git repository with a supported annotation
    When I run zmem-svc add for its path with trusted extensions
    Then the canonical repository is registered once
    And its current HEAD is indexed with extension trust

  Scenario: Add rejects a non-repository
    Given a path outside any Git repository
    When I run zmem-svc add for that path
    Then registration fails without an anchor or entries

  Scenario: Cold query defers indexing and later returns the exact HEAD
    Given a registered repository whose HEAD advances
    When a client queries the new HEAD
    Then the service reports indexing and an explicit retry returns that HEAD

  Scenario: Duplicate cold queries share a job before host identity resolves
    Given a cold repository with a slow first identity host
    When two clients query its exact HEAD during identity validation
    Then both receive one indexing job before the identity host finishes

  Scenario: Status identifies a running service
    Given no zmem service is running for the isolated user home
    When an authorized local client ensures and inspects the service
    Then status reports one running release with its protocol identity

  Scenario: Incomplete client frame does not block health
    Given no zmem service is running for the isolated user home
    When a client leaves an incomplete request frame open
    Then another client can inspect the running service promptly

  Scenario: Framing time counts toward the request deadline
    Given no zmem service is running for the isolated user home
    When a client delays a short-budget request frame
    Then the service reports timeout instead of a late success

  Scenario: Startup lock wait consumes the original deadline
    Given a fresh startup lock held in the isolated home
    When an ensure request has a 100 millisecond budget
    Then startup returns a typed timeout within that budget

  Scenario: Alternate home contains all service state
    Given an alternate zmem home and a separate unused default home
    When an authorized local client ensures the service
    Then service state exists only beneath the alternate home

  Scenario: Installed binary uses its sibling persistent host
    Given a service binary assembled with a sibling Python host
    And a Git repository with a supported annotation
    When I query through the assembled service without a host override
    Then the supported annotation is indexed by the sibling host

  Scenario: Concurrent clients converge on one service
    Given no zmem service is running for the isolated user home
    When two authorized clients ensure the service concurrently
    Then both clients observe the same healthy service identity

  Scenario: A delayed ping does not create a second daemon owner
    Given a daemon whose ping is deliberately delayed
    When two short-budget ensure calls overlap that ping
    Then the original daemon remains the sole healthy owner

  Scenario: HEAD advanced before query
    Given a registered repository whose observed HEAD has advanced
    When that exact observed commit is queried
    Then the service returns an immutable trail through that commit

  Scenario: Query a non-checked-out ref
    Given a resolvable tag, branch, or commit that is not checked out
    When the selector and observed identity are queried
    Then the compatible trail is returned without modifying the worktree

  Scenario: First and repeated shallow demand do not trigger prefetch
    Given eight commits with a requested three-commit view
    When the requested view is indexed through an explicit retry
    Then only the requested facts exist without speculative backfill

  Scenario: Zero ceiling disables automatic prefetch
    Given eight commits with background collection disabled
    When the requested view is indexed through an explicit retry
    Then no background facts or checkpoint are stored

  Scenario: Qualified older demand prefetches one lead window and reuses it
    Given 1600 commits and persisted qualified demand for a 1000-commit route
    When the daemon resumes eligible demand
    Then prefetch stops at 1500 and an explicit wider query reuses 500 speculative facts

  Scenario: Expired demand does not resume an eager checkpoint
    Given 1600 commits and persisted qualified demand for a 1000-commit route
    When those observations expire before restart
    Then the daemon leaves older history uncollected

  Scenario: Failed indexing requires an explicit retry after restart
    Given a repository whose extension expansion fails
    When its cold query admits an indexing job that fails
    Then restart preserves the failure until an explicit job retry succeeds

  Scenario: Cached lookup keeps a host permit during heavy indexing
    Given a published trail and a separate slow indexing host
    When the slow indexer occupies heavy host capacity
    Then the exact cached lookup and health request finish before that indexer

  Scenario: Add and fast check share a cold history job
    Given a published trail and a separate slow indexing host
    When add and fast check wait for the same cold history
    Then both complete while cached lookup remains available

  Scenario: Disconnect cancels a request-owned deep check
    Given a published trail and a separate slow indexing host
    When a deep-check client disconnects after its host starts
    Then the abandoned host exits and another client queries the same daemon

  Scenario: Disconnect cancels lookup identity validation
    Given a published trail and a separate slow indexing host
    When a cached-lookup client disconnects during identity validation
    Then the abandoned host exits and another client queries the same daemon

  Scenario: Disconnect cancels add admission identity validation
    Given a published trail and a separate slow indexing host
    When an add client disconnects during admission identity validation
    Then the abandoned host exits and another client queries the same daemon

  Scenario: Fast check reserves proposed nodes before history admission
    Given a cold repository with one historical decision
    When a proposed decision consumes the entire fast-check node budget
    Then no historical expansion or wider trail is published

  Scenario: A host that never reads a large request is cancelled
    Given a published trail and a separate slow indexing host
    When a large deep check times out writing to an unread host pipe
    Then the timed-out host exits and another client queries the same daemon

  Scenario: Deep-check timeout leaves the daemon usable
    Given a published trail and a separate slow indexing host
    When a deep check exceeds its request deadline
    Then the timed-out host exits and another client queries the same daemon

  Scenario: Shutdown waits for owned work to stop
    Given a published trail and a separate slow indexing host
    When the slow indexer occupies heavy host capacity
    And the service stops while that host is running
    Then its host exits and a new daemon acquires ownership


  Scenario: A saturated writer does not delay cached lookup observations
    Given a published trail and a separate slow indexing host
    When the canonical writer is blocked while cached lookups complete
    Then the lookups succeed and advisory observations persist after the lock clears
