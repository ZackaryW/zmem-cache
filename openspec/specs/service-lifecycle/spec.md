# service-lifecycle Specification

## Purpose

Defines the cross-platform per-user service boundary used to register repositories and answer local zmem clients continuously.

## Requirements

### Requirement: Service runs per user on supported platforms
`zmem-svc` SHALL run as an always-on per-user background service on Windows, macOS, and Linux, SHALL accept requests only through a local user boundary, and SHALL expose its release and protocol identity to authorized local clients. An installed service SHALL run from a stable versionless runtime path and SHALL remain demand-startable when per-user startup registration is absent.

#### Scenario: Client starts a stopped service
- **WHEN** an authorized local client requests the service and it is not running
- **THEN** the client can start it and establish a local connection

#### Scenario: Client inspects a running service
- **WHEN** an authorized local client requests service status
- **THEN** the response identifies whether the service is running and reports its release and protocol versions

### Requirement: Service state can be isolated under an alternate home
The service SHALL honor an explicit alternate zmem home for service state, configuration, extensions, and database files without reading or writing the default user home.

#### Scenario: Service runs under a temporary home
- **WHEN** a client starts and queries the service with an alternate zmem home
- **THEN** all service-owned state is created beneath that home and the default installation is unchanged

### Requirement: Installed runtime supplies a persistent extension host
When launched from an assembled runtime, the service SHALL use the persistent Python extension host associated with that runtime unless the user explicitly configures another host.

#### Scenario: Disposable client environment exits
- **WHEN** the client process that installed or started the service has exited
- **THEN** subsequent indexing uses the extension host stored in the stable runtime

### Requirement: Concurrent startup produces one service owner
The service SHALL coordinate competing start attempts so that one healthy per-user service owns the active state record.

#### Scenario: Two clients start a stopped service
- **WHEN** two authorized clients attempt to start the same stopped service concurrently
- **THEN** they converge on one healthy service identity instead of leaving competing daemons or corrupt state

### Requirement: Add registers exactly one repository
`zmem-svc add <path>` SHALL canonicalize and validate one Git repository, record its extension-trust choice, perform an initial index, and be idempotent for the same canonical repository.

#### Scenario: Add a repository with trusted extensions
- **WHEN** a valid repository is added with `--trust-extensions`
- **THEN** its configured repository extension root participates in the initial index

#### Scenario: Add a non-repository
- **WHEN** add receives a path that is not inside a Git repository
- **THEN** registration fails without creating repository, trail, or entry state

### Requirement: Queries observe the selected HEAD
Before answering a repository query, the service SHALL resolve the requested Git commit-ish or observed worktree HEAD, compare it with the client's observed OID, and select or construct a compatible immutable trail through that exact commit. Resolution, synchronization, or stale-ref failure SHALL return a structured error without returning another trail.

#### Scenario: BDD target — HEAD advanced before query
- **WHEN** executable behavior is covered by `features/service-lifecycle/service-lifecycle.feature::HEAD advanced before query`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Query a non-checked-out ref
- **WHEN** executable behavior is covered by `features/service-lifecycle/service-lifecycle.feature::Query a non-checked-out ref`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps
