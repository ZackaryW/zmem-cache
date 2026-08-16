# service-distribution Specification

## Purpose

Defines the versioned, platform-addressable, integrity-checked release contract through which clients acquire native `zmem-svc` artifacts.

## Requirements

### Requirement: Service releases expose a typed platform manifest
Every published service release SHALL include one strict JSON manifest identifying its release, protocol, and schema versions and exactly one named artifact, byte length, and SHA-256 digest for each supported target. Artifact names SHALL be safe single path components, target identifiers SHALL be unique, and integrity digests SHALL use lowercase hexadecimal SHA-256.

#### Scenario: Inspect a complete service release
- **WHEN** a downstream client reads the manifest for a published service version
- **THEN** it can select one artifact by supported Rust target and verify the artifact without platform-specific filename inference

#### Scenario: Reject malformed release input
- **WHEN** release metadata contains an unsafe name, duplicate target, invalid digest, nonpositive size, or an unknown field
- **THEN** manifest production fails instead of publishing ambiguous metadata

### Requirement: Release identity is coherent
The release tag, Cargo workspace version, service-reported release version, and manifest release version MUST identify the same semantic version before publication. The release manifest's protocol and schema versions MUST equal the values reported by the built service artifacts.

#### Scenario: Refuse a mismatched release
- **WHEN** the requested release tag or any built artifact identity disagrees with the workspace or manifest identity
- **THEN** release assembly fails before GitHub Release publication

### Requirement: Supported targets are published together
A successful release SHALL publish service artifacts for Windows x86-64, Windows ARM64, Windows x86, macOS x86-64, macOS ARM64, Linux x86-64 musl, and Linux ARM64 musl together with the manifest under tag `v<release-version>`.

#### Scenario: Tagged release supplies the platform set
- **WHEN** the coordinated release workflow succeeds for a version tag
- **THEN** the GitHub Release contains every required target artifact and the manifest that describes them
