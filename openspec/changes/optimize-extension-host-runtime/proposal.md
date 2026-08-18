## Why

The service currently starts one Python process per host request and may fan out fifty three-pipe children at once, exhausting a daemon's inherited file-descriptor limit or hanging forever on one host. Routine checks also repeat parser-only work for unchanged commits, multiplying process startup cost without changing results.

## What Changes

- Supervise every extension-host process with bounded concurrency, a configurable 30-second deadline, deterministic cleanup, and structured failures that preserve the current anchor.
- Retry a failed parser-only inspection or identity request at most once after cleanup; never retry expansion that may execute externally visible hooks.
- Cache commit inspection results under their parser/protocol identity and batch uncached parser inspections through the typed host boundary.
- Reduce the default host-process concurrency to a resource-conservative bound while retaining a positive configuration override.
- Preserve deterministic commit application, extension identity validation, and the existing one-request host isolation model; persistent workers remain deferred.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `extension-coordination`: Require supervised, deadline-bounded host execution, safe cleanup, operation-specific retry semantics, and typed batched inspection.
- `repository-indexing`: Reuse valid inspection results and apply a resource-conservative default concurrency without changing bounded-history selection or deterministic application.

## Impact

This changes `zmem-svc` host execution, configuration, inspection selection, SQLite schema, protocol validation, and public failure behavior. It coordinates with the Python `zmem` change of the same name, which owns batched parsing and independently versioned compatible-service acquisition.
