## Why

`uvx` environments are disposable and cannot safely own an always-on daemon or its Python extension host. zmem needs a stable per-user runtime that can be installed, inspected, upgraded, rolled back, and exercised under an isolated temporary root on every supported platform.

## What Changes

- Add a versioned service handshake and structured runtime status suitable for management by the Python `zmem` package.
- Run the installed daemon and extension host from stable, versionless paths under `~/.zmem/runtime/`, with release and compatibility metadata in `runtime.json`.
- Support staged installation and upgrade with one previous runtime retained until the replacement passes its health check.
- Support explicit alternate zmem homes and runtime roots so tests and temporary deployments never mutate the real user installation.
- Support per-user registration adapters for Windows, macOS, and Linux while retaining demand-start operation when no registration exists.
- Preserve the existing local-only service boundary, repository synchronization, and indexing semantics.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `service-lifecycle`: Extend the per-user service contract with runtime identity, stable installed paths, isolated roots, registration lifecycle, and safe replacement behavior.

## Impact

- Affects `zmem-svc` command handling, service state and handshake payloads, runtime path resolution, process startup, and cross-platform registration integration.
- Coordinates with the Python `install-service-runtime` change, which owns the public `zmem service` commands and runtime assembly.
- Does not migrate or reuse the legacy zmem graph or cache.
