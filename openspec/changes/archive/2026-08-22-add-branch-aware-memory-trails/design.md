## Context

`zmem-cache` currently canonicalizes a worktree root, stores one anchor per repository, and materializes effective score and validity directly on repository-owned entries. Queries always synchronize the worktree HEAD, so a branch switch replaces the projection. This prevents simultaneous branch-specific DECAY, CANCEL, and META state and makes repeated branch histories expensive to reconstruct.

The paired Python change introduces live `--ref`, affected-area filters, and a protocol-v4 META effect. Existing cached entries must remain usable as null/global without a migration-time history replay.

## Goals / Non-Goals

**Goals:**

- Represent each compatible bounded projection as an immutable trail selected by a resolved Git commit.
- Share immutable commit and annotation facts while keeping reachability-dependent effects and metadata per trail.
- Resolve branch names live and use cached aliases only as accelerators.
- Derive compact affected areas once for commits newly entering the cache and preserve existing rows as legacy global memory.
- Migrate the current anchor and materialized state into an initial trail without mandatory replay.
- Retain transactional effect application, deterministic attention, sole-writer authority, and bounded storage.

**Non-Goals:**

- Globally identifying commits by OID without repository context.
- Persisting branch names as authoritative history identities.
- Dedicated trail-management commands or a Git-authored TRAIL annotation.
- Eagerly normalizing every legacy effective entry into reusable base facts during migration.
- Supporting database downgrade after schema activation.

## Decisions

### Treat trails as immutable projection identities

A trail is keyed by repository, resolved HEAD OID, attention identity, extension identity, and protocol/schema identity. It stores the deterministic selected commit membership and sparse trail-specific entry state. Local branch aliases point to a last-observed trail but are re-resolved before use. Any Git commit-ish can select a trail; only local branches receive named reusable aliases.

### Scope commit identity to the repository

Commit and expansion facts use `(repository_id, commit_oid)` plus the applicable parser/extension identity where behavior depends on it. The same OID is shared by multiple trails within a repository. Cross-repository sharing remains limited to already safe parser inspections because repository extensions and trust can change expansion behavior.

### Separate shared facts from trail state

Shared facts contain immutable Git metadata, original entry-producing expansion facts, conventional scope, and intrinsic affected areas. Trail membership determines visibility. DECAY/CANCEL results, META overlays, conflicts, and reachability-dependent diagnostics are stored sparsely by trail. This prevents an effect reachable from one branch from mutating another branch's view.

The first implementation reuses existing commit, parser-inspection, and validated entry facts. It does not add a general expansion-journal cache until profiling proves host replay remains material.

### Derive compact affected areas only on first fact insertion

Changed paths are read from Git when a commit first becomes a new shared cache fact. Root files form `<root>`; other paths are grouped by their first component and reduced to each group's deepest common parent. Both rename endpoints participate. Up to three results are stored; a broader result is null/global. Once written, intrinsic areas are immutable.

### Layer META over intrinsic metadata

The protocol-v4 host emits a validated metadata-patch action with endpoints and ordered typed operations. The service resolves the inclusive ancestry-path range in the selected trail and applies the complete patch transactionally to trail metadata. The built-in keys are `affected_areas`, `owner`, and `tags`; canonical entries and intrinsic facts are not rewritten. A META `affected_areas` value overrides the intrinsic value for that trail.

### Migrate the existing projection into a legacy trail

Schema migration creates trail, membership, shared-fact, and sparse-state structures. The current repository anchor and materialized entries become an initial immutable legacy trail. Existing affected areas are null/global. No migration-time Git replay is required. If another trail later needs a legacy commit's missing reusable base fact, it is recovered lazily from that trail's bounded selected history and then reused.

### Preserve exact ref observations

The request carries the selector and client-observed OID. The service resolves the selector again. A mismatch returns a stale-ref error and changes no cache state. A compatible trail is reused; a fast-forward may reuse shared facts and incrementally create a new trail; a rewrite or identity change creates a different trail without deleting facts still referenced elsewhere.

### Retain trails under the existing capacity discipline

Unreferenced trails are eligible for eviction. Trail membership and sparse state are removed before shared facts, and a shared fact remains while any retained trail references it. Recent source-commit protection and deterministic committer-time ordering remain authoritative. Local aliases never keep a stale trail alive after the branch resolves elsewhere unless ordinary retention still selects it.

## Risks / Trade-offs

- [Trail membership duplicates bounded commit identifiers] → Keep entry and commit facts shared; add persistent-delta trails only if measurement justifies the complexity.
- [Legacy trails lack reusable original facts for already-mutated entries] → Preserve exact materialized legacy state and recover base facts lazily only when another trail requires them.
- [META conflicts across merged branches cannot use arbitrary replay order] → Detect incomparable conflicting assignments and require a descendant resolving META.
- [Ref movement races can return the wrong branch state] → Compare the client's observed OID with a fresh native resolution before trail selection.
- [Schema activation prevents old binaries from opening the database] → Use the existing exact client/native compatibility gate and publish the native release first; do not promise downgrade.
- [Retention can orphan reusable facts or replay effects] → Enforce foreign keys and delete unreferenced trail state before considering shared-fact eviction.

## Migration Plan

1. Add the new schema in one transaction and preserve repository registration, trust, inspection records, commits, entries, relationships, and diagnostics.
2. Create one legacy trail per existing anchor and attach its current materialized membership/state without running Git history.
3. Mark existing affected areas null/global and promote compatible runtime metadata to the new schema identity.
4. Start protocol-v4 service handling; resolve new refs into immutable trails and populate reusable facts lazily.
5. Publish native artifacts and strict manifest before the paired Python release.

If migration fails, the transaction leaves the previous database unchanged. After successful schema activation, rollback to an older binary is unsupported; recovery uses the retained database backup policy or reconstruction from Git with the compatible service.

## Open Questions

None.
