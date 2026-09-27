# Adaptive implementation verification

## Compatibility and prerequisite

The predecessor's four reopened tasks were corrected and verified before adaptive implementation. Its corrected binary is retained at `target/corrected-predecessor/zmem-svc.exe` (SHA-256 `83b059e750400e27f53846700a852ced8610a3c48122fc0f9fe37361c40c5f2e`); the source manifest and original HEAD are recorded in the predecessor's verification note. Neither change is committed or released.

This change keeps protocol 5 and advances schema to 6. The companion `handle-bounded-service-lookups` runtime and fixtures were updated through its workflow; 123 client unit tests pass. Supported-Unix process cleanup remains an explicit outstanding task in that companion change, rather than a claim based on Windows results.

The final adaptive binary is `target/adaptive/debug/zmem-svc.exe`, SHA-256 `a5e833efd70bb1efeb695b136b3a1a203d31dc5b29826219e481c16bf7571246`. A final review fixed eager snapshot allocation under advisory backpressure: queue capacity is now reserved before copying, failed submissions retain observations and respect the five-second retry interval, and startup loading leaves accumulator headroom. `advisory_backpressure_does_not_build_an_extra_snapshot` verifies that full or contended queues never invoke the copy closure. Workspace tests, clippy with warnings denied, and formatting pass for this binary.

The transactional migration rekeys compatible schema-5 trails and their dependent rows without replay, preserves failed jobs and checkpoint data, and creates no popularity. Identified conflicting pre-schema-6 advisory tables are replaced under the user's explicit permission. Unknown newer schemas and incompatible views are rejected, with rollback preserving the old schema and failures. No live user database was removed.

## Evidence map

- `crates/zmem-core/src/demand.rs`: controlled-clock policy tests cover shallow and annotation-limited views, interval qualification, repeated 1000-to-1500 stability, cap/zero behavior, expiration and future timestamps. Explicit unlimited demand remains outside the policy and retains existing attention behavior coverage.
- `crates/zmem-core/tests/trails.rs`: HEAD/full-name/short-name aliases, branch advance/switch, detached OIDs, exact observed-identity rejection. Route normalization replaces the existing ref-classification Git operation; it does not enumerate history.
- `crates/zmem-svc/src/main.rs` unit tests: bounded queue/permit admission, same-OID coalescing, durable failure retention through capacity/restart, generation-sensitive pending-job matching and preservation of the original admission route. The route is advisory and excluded from the persisted job key, so migration does not authorize failed-work retries.
- `crates/zmem-svc/src/demand.rs` and store `adaptive.rs`: nonblocking contention/drop behavior, bounded tracked bytes/routes, interval coalescing, persistence expiry/rollback, safe loss of advisory records, and independent durable failure records. A crash can lose the unflushed accumulator by design; a cache with no surviving observations starts without promotion.
- `features/service-lifecycle`: first and repeated shallow use produces no speculative job; a controlled persisted eligible 1000-commit route resumes only through 1500 and a wider demand reuses 500 speculative facts; expired demand does not resume an eager checkpoint. A real external SQLite writer lock leaves cached lookups responsive and advisory flushing recovers afterward. Existing query/add/fast-check sharing, pending/failure handling and isolated deep-check scenarios pass.
- `crates/zmem-svc/tests/prefetch_turns.rs`: 64-commit turns under continuous foreground demand, extension of a validated prefix, missing-fact recollection, named-ref operation after HEAD moves, and a moved high-demand alias cannot lend its target to a surviving lower-demand alias.
- Store `adaptive.rs`, `prefetch.rs` and `existing_connection.rs`: never-used-first LRU, first reuse counted once, retained/shared and active-job protections, recent raw facts, all-protected quota pause, invalidated checkpoints, bounded pins and coherent reader snapshots. Reclamation does not touch index job failures. Canonical source-time retention behavior remains covered by the existing reused-old-fact scenario.
- Diagnostic accounting is writer-owned and separate from lookup response delivery. `prefetch_metrics` counts production/reuse/unused eviction per cache residency; `speculative_usage` supplies remaining unused bytes. Job diagnostics include dropped observations, advisory persistence failures, and existing queue/work timing. Foreground publication records actual reuse atomically; cached-hit reuse is advisory and batched.

## Validation status

### Scenario coverage cross-reference

| Delta scenarios | Executable evidence |
| --- | --- |
| Recent trail without full backfill; frequent shallow access | Lifecycle `First and repeated shallow demand do not trigger prefetch`; mixed-workload assertions |
| Background facts exceed attention; explicit demand exceeds ceiling | Memory-trail cross-window fixtures; existing attention tests and zero-ceiling lifecycle scenario |
| Restart during backfill; reclaimed history requested again | `prefetch_turns.rs` surviving-prefix and missing-fact restart assertions; qualified-demand lifecycle restart |
| HEAD moves; HEAD advance and branch switch | Core trail identity fixtures; lifecycle HEAD/ref scenarios; prefetch multi-ref target assertions |
| Repeated older use; no runaway promotion | Policy `separate_intervals_justify_only_one_lead_window`; qualified-demand lifecycle and benchmark 1000→1500 assertions |
| Annotation-limited selection; inactive demand expires | Controlled-clock policy tests; expired-demand lifecycle restart |
| Polling/pending retries; successful burst | Shared-job lifecycle fixtures; tracker coalescing unit test; successful-completion-only observation sites |
| Saturated advisory recording; metadata quota | Tracker contention/size unit test; lazy advisory queue admission unit test; external SQLite writer-lock lifecycle scenario; store metadata limits |
| Cold demand during prefetch; multiple eligible routes | `prefetch_turns.rs` foreground-load/64-commit boundary assertions; rotating coordinator; mixed-route one/eight-client benchmark |
| Cross-window effects | Native CANCEL/META feature scenarios; annotation-bearing merged benchmark assertions |
| Recently reused old fact; popular old canonical fact | Existing cache-retention feature; unchanged source-time canonical retention path; reuse updates only speculative recency |
| Cold/hot competition; shared fact protected; only protected data | Store LRU, actual first-reuse, active-job/recent-raw tests; existing read-only snapshot tests |
| Restart after inactivity; upgrade existing cache | Expired-demand lifecycle; populated schema-5 migration test; stopped-daemon rollout rehearsal |
| Failed job unpopular | Store advisory expiry/reclamation tests preserve failures; service failure-capacity/restart unit test and explicit-retry lifecycle scenario |

All eight native feature directories passed together: **90 scenarios, 291 steps**. After the final multi-ref target, original-admission-route and advisory allocation corrections, workspace tests, clippy with warnings denied, formatting, and the three affected lifecycle/memory-trail/metadata directories pass again: **46 scenarios, 141 steps**. An earlier rerun was interrupted by a Windows process-launch failure; the complete rerun passed after confirming no service/test processes remained. Strict validation passes. OpenSpec prints the expected informational synchronization warning because the modified priority-tier requirement is introduced by the still-unsynchronized predecessor.

The schema-6 companion pair passed **123 client unit tests and 60 behavior scenarios / 217 steps**. `python benchmarks/rollout_rehearsal.py` then passed against the final native binary in a disposable home: schema 5→6 retained all four exact entries and a durable failure, the older writer rejected schema 6, and stopped-daemon backup restore returned the same entries under schema 5. No live cache was touched.

The comparison workload is in `benchmarks/adaptive_workload.py`, invoked through `tiered_history.py`. It uses three annotation-bearing merged repositories, a tight quota, concurrent deep replay and one/eight lookup clients, plus cross-window CANCEL/META assertions. Promotion setup is a controlled persisted fixture; process deadlines run in real time. Final-binary measurements passed the behavior/utilization assertions in all six repetitions (90 warm samples per client-count group). Full quantiles, variability, initial/wider latency and raw report paths are in [the benchmark report](../../../../benchmarks/README.md).

The separate predecessor utilization audit passed: 2800 distinct speculative facts / 760810 bytes collected, 500 facts / 135962 bytes subsequently demanded (17.86% fact utilization, 17.87% byte utilization). It leaves 26932 unused bytes after the wider demand. This audit explicitly settles cohorts to observe them before reclamation; its timings are excluded from the matched latency table. Raw evidence is in `benchmarks/results/predecessor-utilization.json`.

All final adaptive runs produced and reused 500 facts / 135962 bytes, with zero remaining unused bytes and zero unused eviction. The one-client latency investigation retained the original 242.6 ms baseline mean per-run p95, the adaptive 293.7 ms result, and an unchanged-baseline recheck at 282.7 ms. Both binaries exhibited occasional high tails, so the initial +21.1% gap did not reproduce in the recheck (+3.9%). Eight-client mean per-run p95 was 856.9 ms adaptive versus 878.1 ms predecessor (−2.4%). This supports useful scan/storage savings without a demonstrated consistent >10% latency regression; it is not a claim of faster warm lookup or compliance with the provisional 250 ms burst target. Wider demanded views still took about 12–14 seconds because expansion remains necessary.

## Release order

The predecessor and this change were synchronized and archived in that order on 2026-09-27. All nine canonical specs passed strict validation after each sync. The adaptive recent-trail scenario retains its predecessor title so the updated behavior replaces the existing scenario without dropping its identity. All 27 predecessor and 20 adaptive tasks were complete; the companion client change remains active for its Unix cleanup CI check.

Publish the native protocol-5/schema-6 release before the matching Python client. Stop the daemon before database backup or replacement. Rollback requires a compatible older binary/client/database backup pair or an isolated rebuilt cache. Setting the speculative ceiling to zero does not roll back the schema. No release, service installation or live-cache reset has been performed.
