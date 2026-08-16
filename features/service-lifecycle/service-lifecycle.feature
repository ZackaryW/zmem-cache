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

  Scenario: Query synchronizes an advanced HEAD
    Given a registered repository whose HEAD advances
    When a client queries the new HEAD
    Then the service indexes through that HEAD before responding

  Scenario: Status identifies a running service
    Given no zmem service is running for the isolated user home
    When an authorized local client ensures and inspects the service
    Then status reports one running release with its protocol identity

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
