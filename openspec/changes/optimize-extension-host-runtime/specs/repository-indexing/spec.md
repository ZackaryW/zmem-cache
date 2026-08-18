## MODIFIED Requirements

### Requirement: Indexing concurrency is globally bounded
The service SHALL bound simultaneous extension-host work across repositories to `max_concurrency`, defaulting to 8 and accepting a positive override from `~/.zmem/config.toml`. The bound SHALL apply to parser inspection as well as commit expansion, while validated results SHALL be applied in deterministic commit order.

#### Scenario: More work than the configured bound
- **WHEN** indexing exposes more ready host work than `max_concurrency`
- **THEN** simultaneous host execution never exceeds the configured number and application order remains deterministic

## ADDED Requirements

### Requirement: Immutable commit inspections are reusable
The service SHALL retain validated parser-inspection results for immutable Git commit identities under the protocol and parser identity that produced them. A later selection SHALL reuse only an exact identity match, SHALL inspect every cache miss through the current host, and SHALL produce the same attention result as fresh inspection. A protocol, parser, or commit identity change SHALL prevent stale inspection reuse.

#### Scenario: Unchanged history is checked again
- **WHEN** a repository command repeats attention selection for commit identities already inspected by the current parser protocol
- **THEN** the command reuses their validated counts without starting parser hosts for those commits

#### Scenario: Parser identity changes
- **WHEN** stored inspection results were produced by a different parser protocol identity
- **THEN** the command ignores those results and obtains current validated inspections before selecting history
