## Why

The native cache service currently has no release contract that a freshly installed Python client can resolve safely. Publishing a typed, checksummed cross-platform artifact set makes `zmem service install` usable without a local Rust toolchain or manually supplied binary.

## What Changes

- Add a tag-triggered and manually dispatchable GitHub Actions release workflow based on the established Saucepan cross-platform build pattern.
- Build named `zmem-svc` artifacts for supported Windows, macOS, and Linux target triples.
- Publish a strict release manifest containing the release, protocol, schema, target, asset name, byte length, and SHA-256 digest.
- Require the Git tag, Cargo workspace version, service-reported release version, and manifest release version to agree.
- Publish the manifest and binaries together on the matching `v<version>` GitHub Release.

## Capabilities

### New Capabilities

- `service-distribution`: Defines the versioned, platform-addressable, integrity-checked GitHub Release contract for native service artifacts.

### Modified Capabilities

None.

## Impact

- Adds release automation, a manifest-generation utility, focused tests, and a public release-contract behavior surface.
- Reuses the existing Rust workspace identity and `zmem-svc version-json` protocol.
- Does not publish or mutate a real GitHub Release during local verification.
