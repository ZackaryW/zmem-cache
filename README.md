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

The Python client pins package version `N` to this repository's tag `vN`; releases do not follow a mutable latest-compatible channel. Publish the native service release before publishing the same-version `zmem` Python distribution.

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
zmem-svc ensure
zmem-svc status
zmem-svc stop
zmem-svc version-json
```

Registration canonicalizes the Git root and is idempotent. A query synchronizes a bounded projection through the repository's current `HEAD` before returning. Compatible complete views fast-forward only the new range; shifted finite windows, history rewrites, schema changes, extension-set changes, and attention-policy changes rebuild that repository transactionally.

`check` simulates a proposed message supplied on standard input. Fast checks synchronize the real bounded `HEAD` projection and roll back the hypothetical successor. `--deep` replays the selected history into isolated temporary storage before the proposed message; `--ref` selects one existing commit to evaluate after its selected ancestors. Both modes run trusted expanders, skip hooks, and return structured effect outcomes without persisting preview state.

## Configuration

Create `~/.zmem/config.toml` to override defaults:

```toml
max_concurrency = 50
max_entries = 3000000
protect_recent_days = 14
# Optional explicit development override:
# extension_host = "zmem-extension-host"
# extension_host_args = []
```

Both limits must be greater than zero. `protect_recent_days = 0` disables recent-history protection.

Repository requests also accept `--commit-limit` and `--node-limit`, defaulting to 500 newest commits and 400 syntactically valid zmem annotations. `-1` disables one dimension. Direct native requests inherit `ZMEM_COMMIT_LIMIT` and `ZMEM_NODE_LIMIT` unless the corresponding flag is explicit. Entry, custom, unsupported, DECAY, and CANCEL annotations each consume node attention; a boundary commit is excluded whole rather than partially applied. Structured results report effective limits, selected usage, truncation, and the reached bound.

`ZMEM_HOME` relocates service state, configuration, extensions, startup locking, and the SQLite database. This is the supported isolation boundary for temporary deployments. The Python manager additionally accepts `ZMEM_RUNTIME_ROOT` so active binaries can be staged outside the data home.

The database is `~/.zmem/db/entries.db`. Capacity counts stored zmem entries, not commits or DECAY/CANCEL effects. Eviction removes complete commit cohorts in oldest Git committer-time order. Protected cohorts are never evicted, so the cache can temporarily report `over_capacity: true`.

## Extensions and trust

Global extensions under `~/.zmem/ext/{expanders,hooks}` are enabled for the user. Repository extensions are disabled unless that repository is added with `--trust-extensions`; their root is `${ZMEM_CUSTOM_EXT_ROOT:-.zmem}` and their implementations live below `extend/` or `overwrite/` and then `expanders/` or `hooks/`.

Expanders receive an `ExpansionContext`, record typed actions, and return `None`. They never receive a database connection. Hooks run after canonical expansion/indexing and are read-only; failures are stored as diagnostics without discarding valid entries.

## Recovery

Stop the service before copying or replacing its files. A damaged cache can be recovered by stopping the service, moving `~/.zmem/db/entries.db` aside, and querying repositories again. The database is derived from reachable Git history; DECAY and CANCEL state is recomputed during rebuild rather than stored as an effect ledger.

## Verification

Run the Rust and behavior surfaces independently:

```console
cargo test --workspace
uv run behave features/service-lifecycle
uv run behave features/repository-indexing
uv run behave features/commit-checking
uv run behave features/cache-retention
uv run behave features/extension-coordination
```
