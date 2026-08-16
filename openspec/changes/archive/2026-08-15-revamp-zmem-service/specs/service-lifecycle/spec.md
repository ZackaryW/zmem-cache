## Purpose

Defines the cross-platform per-user service boundary used to register repositories and answer local zmem clients continuously.

## ADDED Requirements

### Requirement: Service runs per user on supported platforms
`zmem-svc` SHALL run as an always-on per-user background service on Windows, macOS, and Linux and SHALL accept requests only through a local user boundary.

#### Scenario: Client starts a stopped service
- **WHEN** an authorized local client requests the service and it is not running
- **THEN** the client can start it and establish a local connection

### Requirement: Add registers exactly one repository
`zmem-svc add <path>` SHALL canonicalize and validate one Git repository, record its extension-trust choice, perform an initial index, and be idempotent for the same canonical repository.

#### Scenario: Add a repository with trusted extensions
- **WHEN** a valid repository is added with `--trust-extensions`
- **THEN** its configured repository extension root participates in the initial index

#### Scenario: Add a non-repository
- **WHEN** add receives a path that is not inside a Git repository
- **THEN** registration fails without creating an anchor or entries

### Requirement: Queries observe the selected HEAD
Before answering a repository query, the service SHALL synchronize through the client's observed HEAD or return a structured synchronization failure.

#### Scenario: HEAD advanced before query
- **WHEN** the registered repository has a new commit and a client queries that HEAD
- **THEN** the service indexes through that commit before returning results
