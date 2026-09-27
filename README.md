# zmem-cache

`zmem-cache` is the Rust, per-user caching service for zmem. It is the sole writer of the SQLite index and works on Windows, macOS, and Linux. The Python `zmem` package owns annotation parsing and extensions; both processes exchange a versioned, typed action journal.

## Install

Build or install the service with a supported Rust toolchain:

```console
cargo build --release --locked
cargo install --path crates/zmem-svc --locked
```

The Python `zmem service install` command assembles releases under `~/.zmem/runtime`. An installed binary at `runtime/binary/zmem-svc[.exe]` automatically invokes the Python interpreter under sibling `runtime/host` with `-m zmem.host`. `ZMEM_EXTENSION_HOST` or `config.toml` can still select an explicit development host; otherwise source builds fall back to `zmem-extension-host` on `PATH`.

## Releases

Tags named `v<workspace-version>` run `.github/workflows/release.yml`, derived from the established Saucepan release matrix. A manual dispatch accepts the same existing tag. Publication is all-or-nothing across these Rust targets:

- `x86_64-pc-windows-msvc`
- `aarch64-pc-windows-msvc`
- `i686-pc-windows-msvc`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`

Assets are named `zmem-svc-<target>` with `.exe` on Windows. `release-manifest.json` records manifest, release, protocol, and schema versions plus each target's asset name, byte length, and lowercase SHA-256 digest. Release assembly fails if the tag, Cargo workspace, captured service identities, target coverage, or artifact names disagree.

Python clients discover the greatest published stable release whose manifest exactly matches their protocol and schema and includes their platform. Native and Python release numbers are independent. Publish a compatible native service release before publishing a Python distribution that can select it.

For a schema-changing installation, stop the daemon and back up `~/.zmem/db/entries.db` after it exits. Publish the matching native release first, then the Python client that selects its protocol/schema pair; install and start that pair together. To roll back, stop the daemon, restore a matching older binary/client pair and the pre-upgrade database backup, or rebuild the derived cache in a separate empty `ZMEM_HOME`. A schema-5 writer rejects schema 6 and must never be pointed at the migrated database.

Manifest construction can be verified locally without publishing:

```console
uv run python -m unittest discover -s tests -p test_release_manifest.py
uv run behave features/service-distribution
```

## Service lifecycle

`add` and `query` automatically start one loopback-only service for the current user. The service state contains a random client token and lives under `~/.zmem/service.json`.

```console
zmem-svc add /path/to/repository
zmem-svc add /path/to/repository --trust-extensions
zmem-svc check /path/to/repository < COMMIT_EDITMSG
zmem-svc check /path/to/repository --deep --commit-limit 500 --node-limit 400 < COMMIT_EDITMSG
zmem-svc check /path/to/repository --deep --ref HEAD
zmem-svc query /path/to/repository --ref feature/payments --observed-oid <oid>
zmem-svc query /path/to/repository --timeout-ms 2000
zmem-svc job-status <job-id>
zmem-svc job-retry <failed-job-id>
zmem-svc ensure
zmem-svc status
zmem-svc stop
zmem-svc version-json
```

Registration canonicalizes the Git root and is idempotent. A query resolves its requested commit-ish live and reads an exact compatible immutable trail if one has been published. On a miss it admits indexing and exits with a JSON `not_ready` error; it does not wait for history scanning. The error includes `job_id`, `requested_oid`, `stage`, and `retry_after_ms`. Inspect progress with `zmem-svc job-status <job-id>`, then explicitly run the query again. Job status includes `queue_depth`, `queue_wait_ms`, and `work_ms` runtime diagnostics; elapsed values for recovered jobs reset after restart. Its `last_publication` field reports the most recent writer transaction's trail ID, duration in microseconds, and WAL file lengths before and after; it is service-wide, not specific to the queried job. A failed job stays failed across service restarts rather than automatically replaying extension hooks; `zmem-svc job-retry <job-id>` explicitly retries it after the cause is addressed. The client-observed OID guards against refs moving during a request. Local branch aliases are never authoritative; tags, remote-tracking refs, detached commits, and other Git commit-ish values remain selectable. Warm lookups reuse a validated extension identity only when the installed host package, interpreter, config, trust, and extension source snapshot still match byte-for-byte; custom hosts and uncertain Python import paths use live host validation. Trails reuse immutable commit inspection and expansion facts, while branch-specific DECAY, CANCEL, META, diagnostics, and entry state remain isolated.

Native query commands default to a 2000 ms deadline; `ensure`, `status`, `stop`, `job-status`, and `job-retry` default to 1000 ms; `add` and `check` default to 120000 ms. Use a positive `--timeout-ms` on the command to override its default. The native client returns one JSON stderr document on failure, for example `{"code":"not_ready","message":"requested history is indexing","retryable":true,"job_id":"job-<hex>","requested_oid":"<oid>","stage":"queued","retry_after_ms":250}`. `busy`, `timeout`, and `stale_ref` are separate codes. `retry_after_ms` is guidance for an explicit later attempt, not an automatic polling instruction. A timed-out or disconnected client releases its request-owned Git and host children; accepted indexing has its own service-owned lifecycle. Stopping the daemon cancels active work, waits for its workers, and leaves interrupted hook-bearing jobs failed until an explicit retry. Completed background fact batches remain checkpointed for restart.

One SQLite database stores trails, facts, checkpoints, and durable `index_jobs` rows (`queued`, `running`, `ready`, `failed`, or `obsolete`). Cold queries return their job ID promptly. `add` and fast check can join that real-history job through bounded waiters without holding a heavy execution worker while indexing runs; their original deadlines and disconnects still end the request. The service-owned job continues for other clients. Deep check replays in a temporary store and never uses a persistent trail as its answer.

When a compatible commit first enters the cache, its changed paths are reduced to at most three conservative affected areas: root-level files become `<root>`, each top-level directory is reduced to its deepest common changed parent, and both rename endpoints participate. Broader changes use null/global metadata. Existing schema-three projections migrate transactionally into immutable legacy trails; a new query builds a compatible trail under the current protocol, schema, and extension identity. Later `zmem(META)` effects can narrow or reset sparse trail overlays.

`check` simulates a proposed message supplied on standard input. Fast checks synchronize a real bounded `HEAD` trail and roll back the hypothetical successor. `--deep` replays the selected history into isolated temporary storage before the proposed message; `--ref` selects one existing commit to evaluate after its selected ancestors. Both modes run trusted expanders, skip hooks, and return structured effect outcomes without persisting preview state.

## Configuration

Create `~/.zmem/config.toml` to override defaults:

```toml
max_concurrency = 8
lookup_capacity = 2
lookup_queue_capacity = 64
heavy_capacity = 32
connection_capacity = 128
extension_host_timeout_seconds = 30
max_entries = 3000000
protect_recent_days = 14
background_commit_limit = 10000
staging_max_bytes = 268435456
# Optional explicit development override:
# extension_host = "zmem-extension-host"
# extension_host_args = []
```

The concurrency, lookup/heavy/connection capacities, host timeout, entry limit, and staging-byte limit must be greater than zero. `connection_capacity` must exceed the sum of lookup and heavy capacities; limits are 64 lookup workers, 256 heavy admissions, and 1024 framing connections. `lookup_queue_capacity` must not exceed `connection_capacity`. The acceptor bounds framing connections, then dispatches complete requests to fixed pools: two lookup workers and a 64-request queue by default, up to eight heavy workers with 32 total heavy admissions, and two independent control workers with a 16-request queue. Admission beyond a capacity returns structured `busy`. `background_commit_limit = 0` disables automatic prefetch. When `max_concurrency` exceeds one, one host permit stays available for lookup identity validation; heavy indexing and checks use the others. A separate worker collects only demand-qualified raw Git messages and changed paths for pinned commits, in turns of at most 64 commits. First or repeated shallow lookups do not start older-history prefetch. It checkpoints completed batches, rotates between queued repositories after each batch, and never runs expansion hooks speculatively. A request for older history still obeys its explicit attention limits and assembles a separate requested trail. Background collection pauses at the staging-byte quota or when HEAD changes. Each one-request extension host has its stdin closed after the request, its output pipes drained concurrently, and a 30-second default deadline; timeout kills and reaps that exact child. Parser inspection is batched and cached by immutable commit plus parser protocol. `protect_recent_days = 0` disables recent-history protection.

Repository requests also accept `--commit-limit` and `--node-limit`, defaulting to 500 newest commits and 400 syntactically valid zmem annotations. `-1` disables one dimension. Direct native requests inherit `ZMEM_COMMIT_LIMIT` and `ZMEM_NODE_LIMIT` unless the corresponding flag is explicit. Entry, custom, unsupported, DECAY, CANCEL, and META annotations each consume node attention; a boundary commit is excluded whole rather than partially applied. Structured results report effective limits, selected usage, truncation, and the reached bound. A META range is applied only when its complete reachable ancestry is selected; incomplete ranges publish no partial metadata state.

`ZMEM_HOME` relocates service state, configuration, extensions, startup locking, and the SQLite database. This is the supported isolation boundary for temporary deployments. The Python manager additionally accepts `ZMEM_RUNTIME_ROOT` so active binaries can be staged outside the data home.

The single canonical database is `~/.zmem/db/entries.db`. Schema 6 adds bounded advisory demand and speculative-use metadata transactionally. Compatible schema-5 trails are rekeyed without replay, preserving membership, failures and checkpoints. Conflicting pre-schema-6 advisory tables can be replaced; canonical trails and durable failures are preserved. A newer unknown schema is rejected. Older native writers must not open a migrated database. Capacity counts stored zmem entries, not commits or effects; `staging_max_bytes` separately counts prefetched raw facts and parent edges that no retained trail references. Speculative collection pauses at that quota. At most 32 speculative traversals pin staged facts; completed/obsolete checkpoint records are capped at 256. Obsolete jobs reclaim unpinned raw facts, startup reconciles orphaned staging, and quota pressure reclaims eligible speculative cohorts by last actual demand use (never-used first, deterministic ties) before pausing new work. Retained trails, recent data and active demand jobs stay protected; evicted checkpoints are invalidated. Production and polling never refresh use recency. Eviction removes unreferenced trails first in deterministic source-time order, then garbage-collects shared published facts that no remaining trail references. Live aliases, configured recent-history protection, and shared facts required by retained trails are protected, so the cache can temporarily report `over_capacity: true`.

## Adaptive prefetch

One route is a canonical repository and normalized full ref. `HEAD` follows the checked-out branch across advances; branches stay distinct, and detached commits use their OIDs. Exact jobs still include the observed OID, attention and extension/trust generation.

The service credits at most one successful real-history observation per route per 60 seconds, retaining at most 60 for one hour. Two observations with **selected** depth greater than 500 justify `min(background_commit_limit, deepest_selected_depth + 500)`. Repeating a 1000-commit view can justify 1500, never 2000. A nominal 10000-commit limit that selects only 120 commits due to annotations stays shallow. Explicit wider or unlimited demand bypasses popularity and the speculative ceiling; requested attention and effect evaluation never change because of popularity.

A shared indexing job contributes once on successful completion. Successful direct hits are rate-limited; job polling, pending retries, rejected/failed requests and temporary deep replay do not promote routes. The protocol cannot distinguish every successful manual repeat from a successful automated repeat; interval limits and the fixed lead bound constrain both. Fast-check history excludes nodes reserved for its proposal.

Access recording uses bounded in-memory state and can skip observations instead of delaying a lookup. Advisory batches flush about every five seconds through the existing writer only when foreground writes are idle. Tracking is capped at 4096 routes and 8 MiB including queued batches; a crash can lose recent observations. Restart discards expired or future observations and never treats an old eager checkpoint as popularity. Losing advisory records only reduces speculation. Canonical source-time retention and failed-job retry records are independent of speculative LRU.

SQLite `prefetch_metrics` reports collected facts/bytes, first demand reuse, unused eviction and reclamation generation; `speculative_usage` identifies remaining unused bytes. Counts cover each fact's cache residency, so recollection after eviction is new collection work. `job-status` includes skipped advisory observations plus existing queue/work timing. Repeated access to a resident fact is not counted as another first reuse.

## Extensions and trust

Global extensions under `~/.zmem/ext/{expanders,hooks}` are enabled for the user. Repository extensions are disabled unless that repository is added with `--trust-extensions`; their root is `${ZMEM_CUSTOM_EXT_ROOT:-.zmem}` and their implementations live below `extend/` or `overwrite/` and then `expanders/` or `hooks/`.

Expanders receive an `ExpansionContext`, record typed actions, and return `None`. They never receive a database connection. Hooks run after canonical expansion/indexing and are read-only; failures are stored as diagnostics without discarding valid entries.

## Recovery

Stop the service before copying or replacing its files. A damaged cache can be recovered by stopping the service, moving `~/.zmem/db/entries.db` aside, and querying repositories again. Shared commit facts are derived from reachable Git history; trail-specific DECAY, CANCEL, META, diagnostics, and entry state are recomputed during rebuild.

## Verification

Run the Rust and behavior surfaces independently:

```console
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
uv run python -m unittest discover -s tests -p test_release_manifest.py
uv run behave features/service-lifecycle
uv run behave features/repository-indexing
uv run behave features/commit-checking
uv run behave features/cache-retention
uv run behave features/extension-coordination
uv run behave features/commit-metadata
uv run behave features/memory-trails
uv run behave features/service-distribution
```
