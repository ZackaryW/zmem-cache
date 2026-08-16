## Context

See `proposal.md`. The current binary demand-starts itself and records only PID, port, token, and protocol version. `ZMEM_HOME` already isolates derived state, while the extension host defaults to a PATH executable that may disappear with an `uvx` cache environment.

## Goals / Non-Goals

**Goals:**

- Make the daemon self-identifying and safely startable by an installed runtime.
- Preserve `ZMEM_HOME` as the single service-state isolation boundary.
- Resolve the persistent host from the installed binary's runtime before falling back to PATH.
- Serialize competing demand-start attempts without introducing a system-wide service.

**Non-Goals:**

- Assemble Python environments or copy release artifacts; the Python package owns that work.
- Define package-registry publishing or CI release matrices.
- Migrate legacy zmem caches or remove demand-start behavior.

## Decisions

### Keep the service binary independently runnable

`zmem-svc` continues to expose `ensure`, `stop`, and `serve`, and adds non-starting structured status/version information. The Python manager composes these commands rather than linking Rust into Python. This preserves a small recovery surface when the Python client is damaged.

### Derive the installed host from the active executable

When no environment or TOML override is present, a binary at `runtime/binary/zmem-svc[.exe]` looks for the platform Python executable under sibling `runtime/host/` and invokes `-m zmem.host`. Falling back to `zmem-extension-host` preserves source-development behavior.

### Treat `ZMEM_HOME` as the temporary deployment seam

Every child daemon inherits the caller's `ZMEM_HOME`; service state, locks, configuration, extensions, and SQLite data resolve from it. Runtime artifacts may live elsewhere because they are selected by the executable path, not the data-home path.

### Use a per-home startup lock

Clients acquire an exclusive create-new lock before spawning, recheck health after acquiring it, and remove stale locks only after establishing that their recorded owner is not live. This is smaller than a cross-platform IPC dependency and protects the only contested transition.

## Risks / Trade-offs

- [A process can die while holding a file lock record] → Include owner metadata and bounded stale-lock recovery.
- [Runtime layout inference couples the binary to an installation convention] → Use it only as a default after explicit overrides and verify the host exists.
- [Status may observe stale state] → Ping the recorded service and report `running: false` without auto-starting it.

## Migration Plan

Existing PATH installations remain demand-startable. New Python-managed installations stop any old service, start the copied binary under the same data home, verify its handshake, and can return to the prior binary without changing the database.
