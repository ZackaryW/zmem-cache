## Context

The service currently obtains complete `git rev-list` output before expansion, stores anchors without attention identity, and asks the host to parse only while expanding. Correct bounded selection must count canonical annotations before expansion, retain whole-commit application, and ensure bounded persistent projections are never mistaken for complete history. See `proposal.md` and the capability deltas for the accepted behavior.

## Goals / Non-Goals

**Goals:**

- Bound candidate discovery before materializing or expanding unneeded history.
- Share one typed attention policy and selection result across synchronization, queries, and deep checks.
- Preserve deterministic parent-first application, whole-commit atomicity, trust, and hook suppression.
- Keep bounded-view persistence and isolated preview semantically distinguishable from complete history.

**Non-Goals:**

- Replacing concurrency, entry-capacity, or recent-retention controls.
- Limiting extension action fan-out, Git object size, or an explicit `-1` traversal.
- Retaining a backward-compatible native protocol or database schema.

## Decisions

### Use a typed policy and selection report

Core will define limits that accept a positive count or unlimited and an `AttentionPolicy` with defaults 500/400. A selection report carries selected SHAs, observed commit/annotation counts, truncation, reached bounds, and a stable policy/view identity. Request fields are optional so direct native callers can resolve environment defaults; Python normally sends explicit effective values.

This central type is preferred over independent integers threaded through service branches because validation and reporting must remain identical.

### Bound Git discovery at the source

Git candidate discovery will request newest-first history with one sentinel beyond a finite commit limit. Proposed annotations are inspected first and reserve node budget. Candidate commit messages are then inspected newest-first; the selector stops before a whole commit that would exceed remaining nodes, records the reached reason, and reverses the final selection for replay.

This avoids the current unbounded `rev-list` collection. Unlimited mode intentionally uses the complete walk.

### Add parser-only host inspection

The host request protocol will add a parser-only operation returning the canonical annotation count and diagnostics without importing expanders or running hooks. Selected commits are expanded separately under existing trust and concurrency rules. Proposed messages exceeding the node budget fail before expansion.

Duplicating Python annotation grammar in Rust was rejected because custom parser evolution would make limits inconsistent with actual zmem behavior.

### Treat bounded persistent state as a projection identity

The schema/protocol will advance. Anchors will include an attention-view identity derived from effective limits, selected lower boundary, counts, and truncation state. A compatible complete or non-shifting bounded view can fast-forward incrementally. If a finite window shifts, a node boundary changes, policy changes, or a formerly bounded view is requested as unlimited, the service transactionally rebuilds that repository from the newly selected bounded view. Rebuild discards stale rows outside the view.

This favors correctness over attempting inverse DECAY/CANCEL application when old commits leave the window.

### Keep preview selection isolated and diagnostics conservative

Deep proposed-file checking uses the same selector with proposed nodes reserved, then replays the selection into the temporary database and evaluates the virtual commit. If older history was omitted and an effect is unresolved, the service reports incomplete attention; it does not rewrite conclusive invalid-factor, ambiguity, forward-reference, or disallowed-target failures.

This preserves useful semantic failures while avoiding false claims about omitted targets.

## Risks / Trade-offs

- [Finite windows can rebuild often near a boundary] → Fast-forward only while the stored selection remains compatible; otherwise bounded rebuild is capped at the requested attention policy.
- [Parser-only inspection doubles host interactions for selected commits] → Inspection avoids extension imports and remains bounded; later optimization may batch requests without changing the protocol semantics.
- [A single commit can exceed the node budget] → Reject proposed messages or exclude historical boundary commits atomically and expose the reached reason.
- [Unlimited mode can still be expensive] → It is available only through explicit `-1` policy and is visibly reported in results.
- [Schema change invalidates existing derived state] → The database is rebuildable from Git and startup/query reconciliation already handles schema identity changes.

## Migration Plan

Advance protocol and schema identities, deploy matching Python and native releases, and rebuild each registered repository into its default bounded projection on first use. Existing databases remain derived data and require no row migration. Rollback uses the matching previous binary/client pair and permits that version to rebuild its own schema.
