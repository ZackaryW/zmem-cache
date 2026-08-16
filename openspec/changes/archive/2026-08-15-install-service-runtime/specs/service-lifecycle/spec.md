## MODIFIED Requirements

### Requirement: Service runs per user on supported platforms
`zmem-svc` SHALL run as an always-on per-user background service on Windows, macOS, and Linux, SHALL accept requests only through a local user boundary, and SHALL expose its release and protocol identity to authorized local clients. An installed service SHALL run from a stable versionless runtime path and SHALL remain demand-startable when per-user startup registration is absent.

#### Scenario: Client starts a stopped service
- **WHEN** an authorized local client requests the service and it is not running
- **THEN** the client can start it and establish a local connection

#### Scenario: Client inspects a running service
- **WHEN** an authorized local client requests service status
- **THEN** the response identifies whether the service is running and reports its release and protocol versions

## ADDED Requirements

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
