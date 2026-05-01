# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — 2026-04-30 *(unreleased)*

The protocol-foundation milestone. No transport / connection / pool
yet — those land in v0.2 and v0.3. This release ships every wire-protocol
type, the supporting error and configuration surfaces, and the testing
harness used to validate them.

### Added

#### Configuration

- `DaemonServer` and `DaemonServerBuilder` (`src/config.rs`) — fluent
  builder with required-field validation via `BuilderError::MissingField`.
  `DaemonServer` is intentionally **not** `Clone` (because `Password`
  isn't); wrap in `Arc<DaemonServer>` to share across multiple pools.
- `TlsConfig` enum: `Verified` (default — system / webpki roots), `Ca`
  (DER-encoded bytes for self-signed pinning), `Insecure` (skip
  validation — gated by the `insecure-tls` Cargo feature).
- `Password` newtype (`src/password.rs`): wraps `Zeroizing<Box<str>>`,
  intentionally not `Clone` / `Copy` / `Display` / `Serialize` /
  `Deserialize` / `PartialEq` / `Hash`. Debug renders `[REDACTED]`.
  Regression-tested via the `zeroize_clears_buffer` test.
- `DaemonServerSpec` DTO (gated by the `serde-config` feature) for
  loading a `DaemonServer` from any serde format (TOML / YAML / JSON
  per consumer choice). `try_into_server()` decodes base64-encoded CA
  certificates and constructs the `Password` at the boundary.

#### Errors

- `Error` enum (`#[non_exhaustive]`) with eight variants: `Transport`,
  `Server`, `Auth`, `Protocol`, `Decode`, `PoolExhausted`, `Cancelled`,
  `Internal`.
- Wrapper sub-types — `TransportError` (Io / Closed), `ProtocolError`
  (Json / CorrelationMismatch / UnknownResponseType), `DecodeError`
  (Serde / MissingColumn).
- `ServerError` carries `message`, `sqlstate`, `sqlcode`, `job_name`,
  `diagnostics`. Predicates classify common SQLSTATE classes:
  `is_transient` (08xxx, 40001, 57033), `is_constraint_violation`
  (23xxx), `is_authorization` (28xxx, 42501), `is_object_not_found`
  (42704, 42S02), `is_data_type_mismatch` (22xxx).
- `From` conversions for `std::io::Error` and `serde_json::Error`.

#### Wire protocol

- `Request` enum covering all 15 Mapepire operations: `connect`,
  `sql`, `prepare_sql`, `prepare_sql_execute`, `execute`, `sqlmore`,
  `sqlclose`, `cl`, `getversion`, `getdbjob`, `setconfig`,
  `gettracedata`, `dove`, `ping`, `exit`. Bare-form per-variant
  `#[serde(rename = "...")]` overrides on `sqlmore`, `sqlclose`,
  `getversion`, `getdbjob`, `setconfig`, `gettracedata` to match
  sibling-SDK conventions.
- `Response` enum covering 12 server-emitted shapes: `Connected`,
  `Pong`, `Exited`, `QueryResult`, `PreparedStatement`, `SqlClosed`,
  `ClResult`, `Version`, `DbJob`, `ConfigSet`, `TraceData`,
  `DoveResult`, `Error`. Several response variants use snake_case
  auto-rename pending daemon-side validation in v0.2.
- Supporting structs: `QueryResult` (rich result-set body with
  metadata, data rows, paging cont_id), `QueryMetaData`, `Column`,
  `ClMessage`, `ErrorResponse`.
- `IdAllocator` — atomic counter with per-process random prefix
  (subsec_nanos + pid) for collision-free correlation ids across
  multiple `Job` instances.

#### Testing

- 48 unit tests across config, password, error, protocol modules.
- 22 `insta` snapshot tests in `tests/wire_snapshots.rs` pinning the
  exact JSON wire shape of every variant (31 `.snap` files).
- 2 `proptest` round-trip tests in `tests/proptest_round_trips.rs`
  fuzzing arbitrary `Request::Sql` and `QueryResult` payloads through
  serde_json (256 cases each, byte-stable assertion). f64 generator
  filters values that don't byte-stably round-trip (a known
  serde_json limitation at the edges of f64 precision).
- 2 doctests on `lib.rs` demonstrating the builder + request encoding.
- Total: **74 tests**.

#### Project infrastructure

- Dual-license posture (`MIT OR Apache-2.0`).
- `Makefile` with `setup`, `build`, `test`, `lint`, `format`,
  `audit`, `deny`, `coverage`, `fuzz`, `outdated`, `msrv-check`,
  `doc`, `pre-commit`, `pre-pr`, `ci`, `release-check` targets.
- `AGENTS.md` — canonical contributor and AI-assistant guide.
- `SECURITY.md` — vulnerability reporting policy with security
  invariants documented.
- `clippy.toml`, `deny.toml`, `.rustfmt.toml`, `.editorconfig`,
  `rust-toolchain.toml`.
- GitHub Actions CI: `fmt`, `clippy`, `actionlint`, `check` matrix
  (Linux/macOS/Windows × stable/beta), `msrv` (Rust 1.85), `test`,
  `docs`, `audit` (cargo-audit), `deny` (cargo-deny). Concurrency
  cancellation on PR runs; per-job `permissions:` blocks; per-job
  `timeout-minutes:`.
- Daily scheduled `cargo audit` workflow.
- SBOM workflow (`anchore/sbom-action`) producing CycloneDX + SPDX on
  release publish.
- Dependabot for cargo + GitHub Actions.
- Branch protection on `main`: 14 required CI status checks, linear
  history, code-owner reviews, conversation resolution required.
  Auto-merge enabled; admin merge available.
- README badges (CI, Audit, deps.rs, MSRV from Cargo.toml, License).
- PR template, issue templates, CODEOWNERS.

[Unreleased]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.1.0
