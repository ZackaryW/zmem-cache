## Purpose

Defines native bounded-attention selection and reporting shared by repository indexing, queries, and isolated effect simulation.

## ADDED Requirements

### Requirement: Native repository work uses dual attention limits
Repository requests SHALL carry effective commit and node limits, defaulting to 500 newest reachable Git commits and 400 syntactically valid zmem annotation occurrences. Each limit SHALL be a positive integer or `-1`, where `-1` disables only that dimension. `ZMEM_COMMIT_LIMIT` and `ZMEM_NODE_LIMIT` SHALL override built-in defaults, and explicit request values SHALL override the environment independently.

#### Scenario: Explicit node limit with environmental commit limit
- **WHEN** a request supplies a node limit while only the commit limit is inherited from the environment
- **THEN** the service applies those two resolved values and reports them in its structured result

#### Scenario: Invalid attention override
- **WHEN** an environment or request attention value is zero, below `-1`, malformed, or non-integral
- **THEN** the service returns a structured configuration or request failure before traversing Git history

### Requirement: Selection is newest-first and replay is parent-first
The service SHALL select the newest reachable commits until the first effective bound is reached, then replay only whole selected commits in parent-before-child order. Every syntactically valid built-in, custom, unsupported, DECAY, and CANCEL annotation SHALL consume one node-attention unit before expansion; plain prose and hook actions SHALL consume none. If another commit would exceed the node limit, the service SHALL exclude that entire commit and all older history.

#### Scenario: Cancellation consumes attention without consuming capacity
- **WHEN** a selected commit contains CANCEL
- **THEN** CANCEL consumes one node-attention unit and applies its effect while still consuming no stored-entry capacity

#### Scenario: Boundary commit is atomic
- **WHEN** only part of the next older commit's annotations would fit in the remaining node budget
- **THEN** none of that commit is expanded or applied and the result identifies node-attention truncation

### Requirement: Attention state is explicit
Every repository synchronization, query, and check result SHALL identify the effective limits, observed selected commit and annotation counts, whether either bound omitted reachable history, and which bound was reached. A cached projection or anchor SHALL retain enough attention identity to prevent a bounded view from being represented or reused as a complete-history view.

#### Scenario: Unlimited request does not reuse a bounded projection as complete
- **WHEN** a repository previously projected under defaults is requested with both limits `-1`
- **THEN** the service rebuilds or extends the projection as needed and reports complete attention only after all reachable history is included

### Requirement: Proposed messages consume only node attention
A proposed check message SHALL not consume a historical commit unit, but each syntactically valid annotation in it SHALL consume node attention before historical commits are selected. If the proposed message alone exceeds the node limit, the service SHALL reject it without partial expansion or persistent mutation.

#### Scenario: Proposed effect reserves node attention
- **WHEN** a proposed message contains one CANCEL under a node limit of 400
- **THEN** the isolated history selection gathers at most 399 annotation occurrences before evaluating that CANCEL
