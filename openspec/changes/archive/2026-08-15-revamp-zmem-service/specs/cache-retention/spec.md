## Purpose

Defines bounded SQLite persistence that evicts old Git commits predictably while protecting recent memory from capacity pressure.

## ADDED Requirements

### Requirement: Cache uses the per-user database
The service SHALL be the sole writer of `~/.zmem/db/entries.db` and SHALL create its parent directories when necessary.

#### Scenario: First service start
- **WHEN** the database path does not exist
- **THEN** the service creates a usable schema before accepting repository work

### Requirement: Capacity is configurable and rolling
The cache SHALL default to 3,000,000 stored entries and accept a positive `max_entries` override from `~/.zmem/config.toml`. After writes, it SHALL evict eligible source commits until the count is at or below capacity or no eligible commit remains.

#### Scenario: Capacity exceeded by eligible history
- **WHEN** a write places the cache above capacity and old eligible commits exist
- **THEN** whole commits are evicted until the capacity is satisfied

### Requirement: Eviction uses source committer time
Eviction SHALL remove every entry and relationship owned by the eligible commit with the oldest Git committer timestamp, never database modification time. Equal timestamps SHALL use deterministic repository-ID and commit-OID ordering.

#### Scenario: Recently modified old entry
- **WHEN** an old commit's stored entry was recently updated by an effect
- **THEN** it retains its original eviction age and remains older than later commits

### Requirement: Recent commits are protected
Commits newer than the wall-clock cutoff of `protect_recent_days` SHALL be ineligible for eviction. The setting SHALL default to 14 days and `0` SHALL disable protection.

#### Scenario: Protected entries exceed capacity
- **WHEN** protected entries alone exceed `max_entries`
- **THEN** none are evicted and the cache temporarily exceeds the nominal capacity

### Requirement: Eviction preserves anchor correctness
Evicting entries SHALL NOT move a repository anchor backward or cause already indexed effects to be applied twice.

#### Scenario: Query after eviction
- **WHEN** old entries were evicted but the repository HEAD has not changed
- **THEN** synchronization does not replay the anchored range solely to recreate evicted entries
