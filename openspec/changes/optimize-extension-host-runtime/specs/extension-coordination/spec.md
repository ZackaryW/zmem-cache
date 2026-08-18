## ADDED Requirements

### Requirement: Extension host execution is supervised
Every extension-host operation SHALL run under the configured positive deadline, defaulting to 30 seconds. The service SHALL close the request input, continuously collect output, and kill and reap the exact child when the operation times out or its process boundary fails. A fatal host failure SHALL return a structured service error and SHALL NOT advance or partially replace a repository anchor.

#### Scenario: Host exceeds its deadline
- **WHEN** an extension host invoked through a repository command remains running beyond the configured deadline
- **THEN** the command fails with a timeout diagnostic, the host is no longer running, and the repository anchor remains unchanged

### Requirement: Host retries respect side-effect authority
A failed parser inspection or extension-identity operation SHALL be retried once through a fresh supervised process after the failed process is cleaned up. An expansion operation that can execute hooks SHALL NOT be retried automatically because hooks may perform external side effects.

#### Scenario: Parser inspection fails once
- **WHEN** a parser-only inspection fails on its first supervised host attempt and succeeds on its second
- **THEN** the repository command uses the second validated result and starts no third attempt

#### Scenario: Hook-bearing expansion fails
- **WHEN** an expansion that may execute hooks fails after the host starts
- **THEN** the repository command reports the failure without invoking that expansion again

### Requirement: Parser inspection supports typed batches
The service SHALL send parser-only inspection work as versioned batches whose items have stable request identities and SHALL accept only a response containing exactly one valid ordered result for every requested item. Missing, duplicate, reordered, extra, malformed, or incompatible results SHALL fail the batch without being used for attention selection.

#### Scenario: Compatible inspection batch
- **WHEN** selected history contains multiple uncached commit messages
- **THEN** the service obtains their annotation counts through a typed batch and associates every result with its requested commit in order

#### Scenario: Incomplete inspection batch
- **WHEN** a host omits one requested item from its batch response
- **THEN** the service rejects the entire response without selecting history from its partial counts
