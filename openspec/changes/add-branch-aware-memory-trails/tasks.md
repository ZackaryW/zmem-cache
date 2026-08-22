## 1. Schema and Migration

- [ ] 1.1 Add fail-first store tests for protocol/schema migration, legacy-trail creation, preserved repository/trust/entry/effect state, null/global legacy metadata, and transactional rollback.
- [ ] 1.2 Implement the minimum trail, membership, shared-fact, sparse-state, metadata, and ref-alias schema plus the explicit prior-schema migration.
- [ ] 1.3 Migrate each existing anchor/materialized projection into one immutable legacy trail without Git replay and preserve lazy recovery markers for missing reusable facts.

## 2. Git and Affected-Area Facts

- [ ] 2.1 Add fail-first pure unit matrices for root paths, deepest-common-parent compaction, one-to-three area retention, four-area global fallback, deletions, and old/new rename endpoints.
- [ ] 2.2 Extend the Git commit boundary to obtain normalized changed-path facts once when a repository-scoped commit fact first enters the cache.
- [ ] 2.3 Implement immutable affected-area derivation and typed intrinsic metadata without retroactively scanning migrated legacy commits.

## 3. Immutable Trail Selection and Reuse

- [ ] 3.1 Add fail-first core/store tests for trail identity, same-HEAD reuse, compatible fast-forward construction, branch rewrite isolation, complete-vs-bounded identities, and shared-fact reuse.
- [ ] 3.2 Implement live commit-ish resolution with client-observed OID comparison, non-authoritative local-branch aliases, and detached/tag/remote selector support.
- [ ] 3.3 Implement deterministic parent-before-child trail membership and sparse branch-specific DECAY, CANCEL, diagnostic, and metadata state with atomic publication.
- [ ] 3.4 Recover missing legacy base facts lazily only when another trail requires them, while retaining the exact migrated legacy trail state.

## 4. META Protocol and Trail Effects

- [ ] 4.1 Add fail-first protocol and store matrices for set/add/null operations, declared keys and types, canonical-field rejection, inclusive reachable-ancestry ranges, attention incompleteness, and ancestry-based conflict resolution.
- [ ] 4.2 Bump the native protocol/schema identities and validate the host's typed metadata-patch journal action without giving the host database authority.
- [ ] 4.3 Apply complete META ranges transactionally to sparse trail overlays, including merged ancestry, global affected areas, descendant replacement, and explicit post-merge resolution.

## 5. Query, Retention, and Service Boundary

- [ ] 5.1 Extend native query requests and responses with requested selector, resolved OID, trail identity, typed metadata, attention/extension/compatibility identities, and structured stale-ref or conflict diagnostics.
- [ ] 5.2 Add fail-first retention tests for unreferenced-trail eviction, protected/referenced shared facts, deterministic source-time ordering, and no duplicate effect application.
- [ ] 5.3 Implement trail-first eviction and shared-fact garbage collection under existing capacity and recent-history protection.

## 6. Public Behavior Contracts

- [ ] 6.1 Create independently runnable `features/memory-trails/` and `features/commit-metadata/` roots with capability feature files, delegated lifecycle, support entry points, and thin scenario-selected bindings.
- [ ] 6.2 Extend the established repository-indexing, extension-coordination, cache-retention, and service-lifecycle public roots only where their declared requirements change.
- [ ] 6.3 Prove RED through exact affected native CLI scenarios, implement the smallest composed behavior, and run each affected Behave root independently to GREEN.

## 7. Complete Verification and Release Compatibility

- [ ] 7.1 Run `cargo fmt --check`, strict configured Clippy, the complete locked Cargo test suite, Python release-manifest tests, and all capability-owned Behave roots independently.
- [ ] 7.2 Build the locked release artifacts and validate version-json, release manifest protocol/schema identities, migration from the prior database, and a clean native package/release build.
- [ ] 7.3 Verify an end-to-end `zmem --ref ... --area ...` query with the paired Python host before publishing the native release ahead of the selecting Python package.
