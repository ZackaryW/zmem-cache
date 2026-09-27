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
Before answering a repository query successfully, the service SHALL resolve the requested Git commit-ish or observed worktree HEAD, compare it with the client's observed OID, and select a published compatible immutable trail through that exact commit. Compatibility SHALL include effective attention, current extension/trust identity, and protocol/schema identity. A missing compatible trail SHALL admit or join bounded indexing work and return structured `not_ready` promptly without scanning history in the lookup path. Resolution, synchronization failure, or stale-ref failure SHALL return a structured error without returning another trail. Admission exhaustion SHALL return `busy`; deadline expiry SHALL return `timeout`. None of these outcomes SHALL be represented as successful empty, partial, or stale results.

#### Scenario: BDD target — HEAD advanced before query
- **WHEN** executable behavior is covered by `features/service-lifecycle/service-lifecycle.feature::HEAD advanced before query`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: BDD target — Query a non-checked-out ref
- **WHEN** executable behavior is covered by `features/service-lifecycle/service-lifecycle.feature::Query a non-checked-out ref`
- **THEN** that exact feature scenario is the executable authority and this specification does not repeat its steps

#### Scenario: Cold query joins indexing
- **WHEN** concurrent queries request the same unindexed compatible snapshot
- **THEN** they receive `not_ready` referring to the same admitted job, and a later query returns that exact snapshot after publication

#### Scenario: Extension inputs change
- **WHEN** a retained HEAD trail was produced under an extension or trust identity that can no longer be validated as current
- **THEN** lookup returns `not_ready` while current compatibility is established instead of returning the retained trail as compatible

### Requirement: Lookup and control capacity are isolated from heavy work
The service SHALL reserve bounded capacity for snapshot lookups and independent health/control handling that indexing and checks cannot consume. Admission and request framing SHALL be bounded. Heavy work SHALL NOT prevent a cached compatible lookup or health request from completing within its configured deadline, and overloaded admission SHALL return a retryable `busy` outcome without an unbounded queue. Service ownership SHALL remain exclusive even when a ping fails or times out.

#### Scenario: History work occupies heavy capacity
- **WHEN** a cold repository is indexing and another repository has a compatible published trail
- **THEN** a query for the published trail and a health request complete without waiting for that index job

#### Scenario: Client sends an incomplete request
- **WHEN** a connection fails to complete framing before its deadline
- **THEN** it is closed and subsequent authorized health and lookup requests remain serviceable

#### Scenario: Busy service is checked by another client
- **WHEN** a client's health probe expires while a live daemon owns the service
- **THEN** the client does not create a second service owner

### Requirement: Native requests have end-to-end deadlines and cancellation
Native commands SHALL accept a positive `--timeout-ms`, with defaults of 2000 ms for queries, 1000 ms for health operations, and 120000 ms for add/check. One monotonic budget SHALL cover discovery/startup, connection, queueing, execution, and response delivery without being reset by retries or phases. Expiry or disconnect SHALL cancel request-owned work, terminate and reap owned subprocesses, release database resources, and leave the daemon usable. An admitted service-owned indexing job SHALL have an independent bounded lifecycle and SHALL NOT be implicitly duplicated or cancelled by a lookup's departure. Response write failures SHALL remain local to the connection.

#### Scenario: Query budget expires during startup
- **WHEN** startup cannot complete inside the query's remaining budget
- **THEN** the native command returns a structured timeout within that budget with bounded cleanup rather than starting another full timeout period

#### Scenario: Client disconnects before response
- **WHEN** a client leaves during request execution or response writing
- **THEN** its request-owned resources are reclaimed and another client can query the same daemon

### Requirement: Deferred and failed work has a machine-readable lifecycle
The service SHALL expose versioned error codes `not_ready`, `busy`, `timeout`, and `stale_ref` separately from annotation validation and fatal service errors. Errors SHALL include a message and retryability and SHALL include job identity, requested OID, and retry guidance when applicable. Authorized clients SHALL be able to inspect queued, running, ready, failed, or obsolete job state through a deadline-bounded job-status operation. Failed jobs SHALL expose their failure rather than remain indefinitely `not_ready` or retry side-effecting work automatically. Native command failure SHALL preserve the error as one JSON stderr document with nonzero exit status.

#### Scenario: Index job fails after a deferred query
- **WHEN** a queued job encounters a fatal expansion failure
- **THEN** job status reports failure and subsequent requests expose that failure without reporting an invalid annotation solely because service execution failed
