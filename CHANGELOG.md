# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] — 2026-05-04 *(unreleased)*

The pool + transactions milestone. v0.3 adds a `Pool` over `deadpool`, a
`Reserved` transaction handle, public `Executor` and `FromRow` traits, and
the diagnostic methods deferred from v0.2.

### Added

#### Pool

- `Pool` and `PoolBuilder` (`src/pool/`) — sibling-SDK-aligned configuration
  (`max_size`, `starting_size`, `acquire_timeout`, `idle_timeout`,
  `recycle`, `default_page_size`, `parameter_logging`).
- `RecyclingMethod::{Verified, Fast}` — default `Verified` (round-trip a
  ping on checkout to survive IBM i firewalls' silent idle TCP kills).
- `ParameterLogging::{None, TypesAndCount, Full}` — stored in the pool but
  only emitted in v0.4's `tracing` spans.
- `Pool::execute` / `Pool::execute_with` — least-busy-job routing
  (try-idle via `timeout_get(ZERO)` → scan up to `min(status().size, 8)`
  checked-out jobs by `in_flight` → fallback `pool.get().await`).
- `SATURATION_THRESHOLD = 32` — once every Job carries ≥ 32 in-flight
  requests, the routing scan abandons step 2 and falls through to fair
  queueing rather than pile on a saturated dispatcher.
- `Pool::acquire` returning `Reserved` for transactional work.
- `Pool::status()` — passthrough to `deadpool::Status`. `PoolStatus` is
  re-exported from the crate root so callers don't need to depend on
  `deadpool`.

#### Transactions

- `Reserved` (`src/pool/reserved.rs`) — exclusive single-connection handle.
  Derefs to `&Job`. Sentinel `in_flight = u32::MAX` while held so the
  routing scan skips it.
- `Reserved::rollback_on_drop()` — opt-in safety. Drop fires a
  best-effort `ROLLBACK` via the existing `spawn_best_effort` helper.
  Currently unconditional even after explicit `COMMIT` — see deferrals.

#### Public traits

- `Executor` (`src/executor.rs`) — implemented for `&Job`, `&Pool`,
  `&Reserved`. Object-safe; usable as `&dyn Executor`. Methods return
  `Pin<Box<dyn Future<Output = Result<Rows>> + Send + 'a>>`.
- `FromRow` (`src/from_row.rs`) — blanket impl for `T: DeserializeOwned`.
  Hand-implementable for custom column-name mapping.
- `Rows::stream_typed::<T>()` — typed streaming via `FromRow`.
- `Rows::into_typed::<T>()` bound widened from `T: DeserializeOwned` to
  `T: FromRow`. Existing v0.2 callers compile unchanged via the blanket.

#### Diagnostic methods (carry-over from v0.2)

- `Job::set_trace(TraceLevel)` — configure the daemon's trace level.
  `TraceLevel::{Off, Errors, Datastream, All}`.
- `Job::fetch_trace()` — fetch the daemon's accumulated trace data.
- `Job::visual_explain(sql)` — daemon-side `dove` op; returns the raw
  explain plan as `serde_json::Value` (typed parsing deferred to v0.4+).
- `Rows::columns()` — `Option<&[Column]>` accessor for column metadata
  (returns `None` for DML / DDL).

#### Internals

- `Job` refactored to `Arc<JobInner>` so `Weak<Job>` references can survive
  in the pool's routing registry. `Job::version()` and `Job::initial_job()`
  are now method accessors returning `&str` (was `pub` String fields).
- `AtomicU32 in_flight` on `JobInner`, shared `Arc<AtomicU32>` between
  `Job` and the dispatcher task. Incremented when a request is queued for
  sending, decremented when the matching response arrives or on socket-close
  drain.
- `Pool` routing registry — `Mutex<Vec<Weak<Job>>>` populated by
  `JobManager::create`. Opportunistic GC of dead Weaks; sentinels filtered
  out.
- `Type = Arc<Job>` on `JobManager` — load-bearing for the routing
  registry.

### Changed

- `Job::version` and `Job::initial_job` are now methods returning `&str`
  (were `pub` String fields). Crate is unpublished — no compatibility
  break in practice.
- `Rows::into_typed` bound widened from `T: DeserializeOwned` to
  `T: FromRow`.
- `mapepire::Row` field changed from `data` (private) to a `pub(crate) fn from_map`
  constructor. The public `Row::get` / `Row::try_get` / `Row::map` API
  is unchanged.

### Testing

- Three new pool integration suites: `tests/pool_basic.rs`,
  `tests/pool_transactions.rs`, `tests/pool_routing.rs`.
- `tests/common/mod.rs` extended with a multi-connection mock harness:
  `spawn_mock_pool`, `MockHandle::observed_sql`, `observed_socket_ids`,
  `last_socket_for_sql`, `pause_responses`, `open_socket_count`.
- `tests/dispatcher_in_flight.rs` — verifies dispatcher in-flight tracking.
- `tests/manager_smoke.rs` — JobManager create+recycle round-trip.

### Open items deferred to v0.4

- `tracing` spans + `ParameterLogging` enforcement (the value is stored
  but not emitted).
- `metrics` facade integration.
- Tightening `Reserved::rollback_on_drop` to fire ROLLBACK only if a
  `BEGIN` was observed without a matching `COMMIT` (current contract is
  unconditional once opt-in is set).
- `idle_timeout` enforcement via deadpool's runtime hooks (the value is
  stored on `PoolBuilder` but not driven by deadpool yet).
- Real-network recycle robustness — `Pool::execute`'s step-1 try-idle
  uses `timeout_get(recycle: ZERO)` which allows ~1 timer-tick of grace.
  On real IBM i deployments where ping RTT exceeds that, the recycle ping
  may time out → connection thrash. Documented inline at the call site.
- Typed Visual Explain plans (currently `serde_json::Value`).

## [0.2.0] — 2026-04-30 *(unreleased)*

The transport milestone. Adds the full async client stack — TLS, WebSocket
framing, per-`Job` dispatcher, and the complete public API surface (`Job`,
`Query`, `Rows`, `Row`). All v0.2 functionality is exercised by a mock
TLS+WebSocket harness and 10 integration tests; no real IBM i required for
the test suite.

### Added

#### Transport layer (Phase 1)

- TLS connect helper supporting both `rustls` (default, gated by
  `rustls-tls`) and `native-tls` (gated by `native-tls`) backends.
  `TlsConfig::Insecure` requires the `insecure-tls` feature at compile time
  and emits a runtime warning on first use.
- WebSocket framing over TLS via `tokio-tungstenite`.
- Per-`Job` dispatcher task: bounded mpsc(64) outbound queue,
  `tokio::select!` event loop, oneshot-based response correlation by request
  id, cancellation-safe drop semantics.
- High-level handshake (TCP → TLS → WebSocket Upgrade → `Connect` request).
- New runtime dependencies: `tokio` 1 (rt-multi-thread, macros, net, time,
  sync, io-util), `tokio-tungstenite` 0.27 (connect, handshake),
  `futures` 0.3, `pin-project-lite` 0.2, `async-trait` 0.1, `bytes` 1.
  Optional: `rustls` 0.23, `tokio-rustls` 0.26, `rustls-pki-types` 1,
  `webpki-roots` 0.26 (`rustls-tls`); `native-tls` 0.2,
  `tokio-native-tls` 0.3 (`native-tls`).

#### Job API (Phase 2)

- `Job::connect(&server) → Result<Self>` — single-connection handle.
- `Job::ping() → Duration` — round-trip metadata heartbeat.
- `Job::server_version() → String` and `Job::db_job_name() → String`.
- `Job::execute(sql)` and `Job::execute_with(sql, params)` — one-shot SQL.
- `Job::prepare(sql) → Query` — server-side prepared statement.
- `Job::cl(command) → ClMessage` — IBM i CL command (returns first message;
  full typed `CommandResult` deferred to v0.3).
- `Drop for Job` — best-effort `Exit` request via `spawn_best_effort`.

#### Query / Rows / Row (Phase 3)

- `Query::execute(&ids)` / `execute_with(&ids, params)` /
  `execute_batch(&ids, batches)` — sequential batch, fail-fast.
- `Rows::update_count() → Option<i64>`, `has_results() → bool`,
  `execution_time() → Duration`.
- `Rows::stream() → impl Stream<Item = Result<Row>>` — automatic paging via
  `sqlmore` with per-stream `IdAllocator`.
- `Rows::into_typed::<T: DeserializeOwned>() → Vec<T>` and
  `into_dynamic() → Vec<Row>`.
- `Row::get::<T>(column) → Result<T>` and
  `try_get::<T>(column) → Option<Result<T>>`.
- `Drop for Query` — best-effort `sqlclose` via `spawn_best_effort`.
- `crate::job_helpers::spawn_best_effort` helper (pub(crate)) shared by
  `Drop for Job` and `Drop for Query`.

#### TLS bootstrap (Phase 4)

- `DaemonServer::fetch_certificate(host, port) → Vec<u8>` — gated by
  `insecure-tls`. Captures the daemon's leaf certificate as DER bytes for
  subsequent pinning via `TlsConfig::Ca(...)`.

#### Test infrastructure (Phases 5–7)

- Mock TLS+WebSocket server harness (`tests/common/{mod.rs, mock_server.rs}`)
  — self-signed certificates minted via `rcgen`, `MockBehavior` enum with
  variants: `AcceptAndConnect`, `AuthFail`, `Pages`, `PrepareAndExecute`,
  `ReturnError`, `HalfOpen`.
- 9 Phase 6 integration tests: handshake happy path, auth failure,
  SQL one-shot SELECT/DML, prepared statement + batch, paging, concurrent
  multiplexing, cancellation safety, server-side error classification,
  half-open socket.
- 1 Phase 7 integration test: `fetch_certificate` round-trip.
- New dev-deps: `rcgen` 0.14, `tokio-rustls` 0.26 (also a dev-dep),
  `rustls` 0.23 with `ring` provider feature.

#### Documentation

- `AGENTS.md` §13 codifies the multi-agent review cadence: spec-compliance
  and code-quality reviews run on the implementer's local branch before a PR
  is opened; CI is the merge gate, not a review surface.
- `SECURITY.md` updated with the wire-protocol-boundary `Password` leak
  tradeoff (see Security section below).

### Changed

- `Job::ids()` accessor visibility promoted from `pub(crate)` to `pub`
  (with `#[must_use]`) to allow consumer doctests to reference the
  `IdAllocator`. Originally `pub(crate)` in PR #32; promoted in PR #36.
- `webpki-roots` 0.26 `CDLA-Permissive-2.0` license added to `deny.toml`'s
  allow list (Mozilla CA bundle; permissive Linux Foundation license).

### Fixed

- (none — fresh feature surface)

### Security

- `Password::expose()` doc accurately describes the bounded-leak tradeoff at
  the wire-protocol boundary: the `Password` newtype itself remains
  zeroize-on-drop, but `Request::Connect`'s payload clones the plaintext into
  a non-zeroizing `String` that lives until the request is dropped after
  serialization. This is an accepted tradeoff bounded to connection time;
  documented in `SECURITY.md` and the function's `///` doc. A future revision
  may thread `Zeroizing<String>` through `Request::Connect` to close the gap.
- `bans.skip` in `deny.toml` gains `getrandom`, `r-efi`, `wit-bindgen`
  (WASI-only transitives never compiled on supported Linux/macOS/Windows
  targets) and `RUSTSEC-2026-0009` advisory ignore (`rcgen` dev-dep / `time`
  0.3.45 — RFC-2822 parsing path is unreachable in our call sites; verified
  by code-quality review).
- Audit-action workflow updated to ignore `RUSTSEC-2026-0009` on the same
  grounds.

---

**Deferred to v0.3:** `Pool` connection pool and `Reserved` connections for
transactions; `Job::set_trace` / `fetch_trace` / `visual_explain`; full typed
`CommandResult` for `Job::cl`; `Rows::columns()` accessor; `Executor` and
`FromRow` traits; `Drop for Rows` to fire `sqlclose` on cursor drop.

**Wire-tag posture:** v0.2's mock server emits the snake_case wire tags pinned
by v0.1's snapshot suite. Daemon-side validation against a real Mapepire
daemon is deferred to v1.0; if real-daemon tags diverge, `Response` enum
`#[serde(rename = "...")]` overrides and the mock harness must be updated in
lockstep.

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

[Unreleased]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.3.0
[0.2.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.2.0
[0.1.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.1.0
