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
zmem-svc ensure
zmem-svc status
zmem-svc stop
zmem-svc version-json
```

Registration canonicalizes the Git root and is idempotent. A query resolves its requested commit-ish live and publishes an immutable trail identified by repository, resolved HEAD, attention policy, extension identity, and protocol/schema identity. The client-observed OID guards against refs moving during a request. Local branch aliases accelerate lookup but are never authoritative; tags, remote-tracking refs, detached commits, and other Git commit-ish values remain selectable. Trails reuse immutable commit inspection and expansion facts, while branch-specific DECAY, CANCEL, META, diagnostics, and entry state remain isolated.

When a compatible commit first enters the cache, its changed paths are read once and reduced to at most three conservative affected areas: root-level files become `<root>`, each top-level directory is reduced to its deepest common changed parent, and both rename endpoints participate. Broader changes use null/global metadata. Existing schema-three projections migrate transactionally into immutable legacy trails with null/global areas and no Git replay. Later `zmem(META)` effects can narrow or reset those sparse trail overlays without rebuilding historical commits.

`check` simulates a proposed message supplied on standard input. Fast checks synchronize a real bounded `HEAD` trail and roll back the hypothetical successor. `--deep` replays the selected history into isolated temporary storage before the proposed message; `--ref` selects one existing commit to evaluate after its selected ancestors. Both modes run trusted expanders, skip hooks, and return structured effect outcomes without persisting preview state.

## Configuration

Create `~/.zmem/config.toml` to override defaults:

```toml
max_concurrency = 8
extension_host_timeout_seconds = 30
max_entries = 3000000
protect_recent_days = 14
# Optional explicit development override:
# extension_host = "zmem-extension-host"
# extension_host_args = []
```

The concurrency, host timeout, and entry limit must be greater than zero. Each one-request extension host has its stdin closed after the request, its output pipes drained concurrently, and a 30-second default deadline; timeout kills and reaps that exact child. Parser inspection is batched and cached by immutable commit plus parser protocol. `protect_recent_days = 0` disables recent-history protection.

Repository requests also accept `--commit-limit` and `--node-limit`, defaulting to 500 newest commits and 400 syntactically valid zmem annotations. `-1` disables one dimension. Direct native requests inherit `ZMEM_COMMIT_LIMIT` and `ZMEM_NODE_LIMIT` unless the corresponding flag is explicit. Entry, custom, unsupported, DECAY, CANCEL, and META annotations each consume node attention; a boundary commit is excluded whole rather than partially applied. Structured results report effective limits, selected usage, truncation, and the reached bound. A META range is applied only when its complete reachable ancestry is selected; incomplete ranges publish no partial metadata state.

`ZMEM_HOME` relocates service state, configuration, extensions, startup locking, and the SQLite database. This is the supported isolation boundary for temporary deployments. The Python manager additionally accepts `ZMEM_RUNTIME_ROOT` so active binaries can be staged outside the data home.

The database is `~/.zmem/db/entries.db`. Capacity counts stored zmem entries, not commits or effects. Eviction removes unreferenced trails first in deterministic source-time order, then garbage-collects shared facts that no remaining trail references. Live aliases, configured recent-history protection, and shared facts required by retained trails are protected, so the cache can temporarily report `over_capacity: true`.

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
