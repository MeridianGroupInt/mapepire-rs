# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.2] — 2026-08-28

Live leftovers after 0.7.1 paging: `gettracedata` and `dove` untagged
replies, plus one-shot 2-D `prepare_sql_execute`. Patch versus 0.7.1
(decode / additive API, not a breaking cut).

### Fixed

- **`Job::fetch_trace` keeps live `tracedata`.** Untagged
  `{id, success, tracedata}` was classified as `Pong` and the buffer was
  dropped (`unexpected variant: Pong`). Decode on `tracedata` /
  `jtopentracedata` (including `""` / `null`). Omitted-key success remaps
  to empty `TraceData`. Ping `{id, success}` without those keys stays
  `Pong`. OSS-10 was the same bug (duplicate; canceled).
- **`Job::visual_explain` keeps live `vedata`.** Untagged dove
  `{vedata, vemetadata}` became `Pong` (no `data`) or `QueryResult`
  (`run=true` has `data`). Classify on `vedata` / `vemetadata`. Sends
  `run: true` (mapepire-js `ExplainType.RUN`). SQLSTATE 42505 without
  `vedata` is `Error::Server` (profile authority), not a crate fail.

### Added

- **`Job::execute_sets` / `Query::execute_sets`** — one
  `prepare_sql_execute` with 2-D `parameters` (`[[1,"a"],[2,"b"]]`).
  Empty outer list is `ProtocolError::EmptyParameterSets` and is not
  sent. `Query::execute_batch` stays sequential. Single-set
  `execute_with` still flattens `[7]` (OSS-7).

## [0.7.1] — 2026-08-28

Live paging: `Rows::stream` never issued `sqlmore` on stock Mapepire
because sql replies omit `cont_id` and may omit `is_done`. Patch versus
0.7.0 (decode / cursor handle, not a breaking cut).

### Fixed

- **`Rows::stream` pages until `is_done`.** PROTOCOL.md §6 and
  mapepire-js use the opening request `id` as the cursor
  (`sqlmore.cont_id` / `sqlclose.cont_id`). 0.7.0 required a non-empty
  `QueryResult.cont_id` and stopped after the first page of 100.
- **Omitted `is_done` on a result set is not done.** JS treats missing
  as falsy (`RUN_MORE_DATA_AVAILABLE`). DML frames that omit both
  `is_done` and `data` still count as done so we do not `sqlmore` /
  `sqlclose` a non-cursor (SQLSTATE 24000 / OSS-7).
- **`sqlmore` keeps the opening handle.** Follow-up replies echo the
  new request `id`; that is not the cursor.

### Added

- **`Rows::is_done` and `Rows::first_page_len`** for first-page cursor
  state so callers need not infer paging from `n == 100`.

## [0.7.0] — 2026-08-28

Live-daemon leftover after 0.6.1: CL job log, bind page size, terse
rows, CALL/OUT, and trace dest. Breaking versus 0.6.1. 0.6.2 was never
published; this is the cut of OSS-2 through OSS-7.

### Breaking

- **`Job::cl` returns `ClOutcome`** instead of the first `ClMessage`
  (OSS-2). Failed CL is `Ok` with `success: false` and the full job
  log; it is no longer `Err(Error::Server)` that dropped `data`.
- **Default page size is 100** (OSS-4 / OSS-7). `Job::execute` and
  `Job::execute_with` send `rows: 100` (mapepire-js `rowsToFetch`).
  0.6.1 omitted `rows` (daemon default **1000**). Follow-up `sqlmore`
  reuses the opening page size. `rows: 0` is
  `Error::Protocol(ProtocolError::ZeroPageSize)` and is not sent.
- **`Job::prepare` without a server `cont_id` is a client-side `Query`**
  (OSS-7). A live `{id,success:true}` ack (no `cont_id`) succeeds;
  each `Query::execute_with` then sends `prepare_sql_execute`. A real
  `cont_id` still uses the `execute` opcode. Ping `{id,success}` remains
  `Pong`.
- **`Job::set_trace` dest is `IN_MEM`, never `""`** (OSS-6). Empty dest
  is invalid on the wire (Jetty/Gson `No enum constant Tracer.Dest`).
  `TraceDest::{File, InMem}` serialize as `FILE` / `IN_MEM`.
- **`QueryResult` / `QueryMetaData` grow optional CALL/OUT fields**
  (OSS-5). `parameter_count`, `output_parms`, and `parameters`
  (`ParameterDetail` / `ParameterResult`). Extra JSON fields on
  existing result types; empty values omit on serialize.

### Changed

- **Single-set `prepare_sql_execute.parameters` serialize as `[7]`**, not
  `[[7]]`. Batches stay `[[a],[b]]`.
- **`TraceLevel::All` sends `"ON"`** (mapepire-js `ServerTraceLevel`). The
  daemon has no `ALL` constant.

### Added

- **`ClOutcome` and `JobLogEntry`** (OSS-2). Protocol column names;
  `SEVERITY` accepts a JSON number or string.
- **`ExecuteOptions`** (`rows: Option<u32>`, `terse: bool`) (OSS-3 /
  OSS-4). `Job` / `Query` / `Pool` / `Reserved` /
  `Executor::{execute_opts, execute_with_opts}`. `terse: true` sends
  `terse: true` on `sql` / `prepare_sql_execute` / `execute`; default
  `false` omits the field (object rows).
- **`terse: Option<bool>`** on `Request::{Sql, PrepareSql,
  PrepareSqlExecute, Execute, Cl, Dove}` (`skip_serializing_if` none).
  `Job::cl` always omits it so job-log rows stay named objects.
- **`CALL` / OUT parameters** (OSS-5). `QueryResult` keeps
  `parameter_count` and `output_parms`; `QueryMetaData` keeps
  `parameters`. `Rows::{output_parms, parameter_count,
  parameter_metadata}` surface them. Same `prepare_sql_execute`
  opcode; no CALL type and no QCMDEXC helper.
- **`TraceDest::{File, InMem}`** (`FILE` / `IN_MEM`) (OSS-6).
  `Job::set_trace(level)` defaults dest to `IN_MEM` so `fetch_trace`
  can read the buffer. `Job::set_trace_config(dest, level)` matches JS
  `setTraceConfig`. Empty `tracedest` / `tracelevel` omit on serialize
  (leave current).

### Fixed

- **Untagged `success: false` with `data` / `has_results` decodes as
  `QueryResult`** (OSS-2). Live `type: cl` replies are QueryResult-shaped
  job-log rows (`MESSAGE_ID`, `SEVERITY`, …). Bare `success: false`
  without those keys remains `Error`.
- **`QueryResult` keeps `error` / `sqlcode` / `sqlstate`** (`sql_rc` /
  `sql_state` aliases) so CPF0006 frames (`sql_rc=-443`,
  `sql_state=38501`) are not stripped.
- **`execute_with` SQLSTATE 24000** from omitted `rows` on
  `prepare_sql_execute` and from `sqlclose`/`sqlmore` when the cursor
  was already done or had no handle.
- **`prepare` decoded as `Pong`.** Outstanding `PrepareSql` remaps the
  untagged success ack to a prepared statement with no server handle.
- **Terse array-shaped `QueryResult.data` decodes** (OSS-3). Live
  `[[7]]` plus `metadata.columns[0].name == "1"` becomes a named map so
  `Row::get("1")` works. Array rows with no `metadata.columns` are
  `ProtocolError::TerseRowsWithoutColumns` (no panic). Object rows still
  decode.
- **`Job::set_trace` sent `tracedest: ""`.** Dest is now `IN_MEM` (or
  `FILE` via `set_trace_config`); empty string is never serialized.

## [0.6.1] — 2026-08-28

Live-daemon leftover after 0.6.0: parameterized SQL, prepare, and ping.

### Fixed

- **`Job::execute_with` sends `prepare_sql_execute`.** Live daemons ignore
  `?` on `type=sql` (SQL0313). Unbound `Job::execute` is still `type=sql`.
- **Untagged `prepare_sql` replies decode as `PreparedStatement`.** Those
  frames carry `cont_id` (and often `is_done`) without `has_results`.
- **Untagged `{id, success}` decodes as `Pong`.** `getdbjob` `{id, success,
  job}` remaps from `Connected` using the outstanding request kind.
- **One unrecognized JSON object fails that request only.** Invalid JSON
  / I/O / peer close still take the dispatcher down.

## [0.6.0] — 2026-08-27

Wire-protocol and TLS handshake now match a live Mapepire daemon
(mapepire-js 0.6.x). 0.5.1 cannot complete a session against stock
Jetty Mapepire. Breaking versus the 0.5.1 mock dialect.

### Breaking

- **WebSocket path `/db2` → `/db/`.** Live Jetty Mapepire 404s `/db2`
  and 403s `/db/` without HTTP Basic.
- **Connect JSON: `{user,password}` removed; `{technique,application,props?}`
  added.** Handshake always sends `technique: "tcp"`; `application`
  defaults to `"mapepire-rs"`; JDBC properties come from
  `DaemonServer::jdbc_props`.
- **Auth is HTTP Basic on the Upgrade.** HTTP 401/403 map to
  `Error::Auth`. The password is not on the query string and is not in
  `Request::Connect` JSON.
- **`Response` decode accepts untagged `{success,...}` frames.** Live
  daemons omit `"type"`; tagged mock frames still deserialize.
- **`TlsConfig::Ca` is a leaf pin on rustls** (SAN skipped when the
  presented leaf DER matches the pin). A non-matching leaf still uses
  `WebPkiServerVerifier`. `TlsConfig::Verified` and the `native-tls`
  backend are unchanged. Do not use `insecure-tls` for CN-only IBM i
  certs.
- **`DaemonServer` gains `connect_address`, `jdbc_props`, `application`.**
  `base64` is now a required dependency (was `serde-config` only); the
  handshake encodes HTTP Basic.

### Fixed

- **rustls default feature panics without a CryptoProvider.** The
  library `rustls` dep now enables `ring` and installs the provider at
  handshake (ignoring `AlreadyInstalled`).
- **CN-only IBM i certificates with `TlsConfig::Ca`.** rustls
  0.23/webpki refuses CN-only leaves even when they are the trust
  anchor; the leaf-pin path skips name checks on exact DER match.
- **Tunneled deploys that need a TCP address distinct from the cert
  name.** `host` remains SNI, HTTP `Host`, and the certificate name;
  TCP uses `connect_address` when set.

### Added

- `DaemonServerBuilder::connect_address` / `jdbc_props` / `application`
  (and matching optional `DaemonServerSpec` fields under `serde-config`).
- `DaemonServer::fetch_certificate_from(server_name, connect_address,
  port)` for tunneled bootstrap; `fetch_certificate(host, port)`
  delegates with both names equal.

### Security

- Password no longer appears in connect JSON. HTTP Basic is built from
  `Password::expose()` into a Zeroizing buffer and is not logged.

## [0.5.1] — 2026-08-27

### Security

- **`Request`'s `Debug` no longer prints the `Connect` password.** The impl
  is hand-written and renders that field as `[REDACTED]`; every other
  variant and field formats as before. `Serialize` is unchanged — the
  plaintext password is what goes on the wire, inside TLS. Downstream code
  doing `tracing::debug!("{req:?}")` no longer logs an IBM i password.

## [0.5.0] — 2026-08-25

The MSRV milestone. Raises the declared minimum supported Rust version to
**1.88** and removes the three workarounds the old 1.85 floor forced on
us. No breaking API changes — the major-version-shaped bump is the MSRV
raise itself, which `AGENTS.md` §9 classifies as a minor bump.

### Changed

- **MSRV raised from 1.85 to 1.88** (`rust-version` in `Cargo.toml`, the
  `msrv` CI job). Consumers on 1.85–1.87 must stay on `0.4.x`.
- **`tokio-tungstenite` 0.29 -> 0.30.** The TLS feature flags forward
  into it (`tokio-tungstenite/rustls-tls-webpki-roots`,
  `tokio-tungstenite/native-tls`), so the tungstenite / rustls /
  native-tls stacks move together in a single commit to keep
  `cargo-deny`'s `multiple-versions = "deny"` satisfied.
- **`base64` 0.22 -> 0.23.1** (reachable only under `serde-config`, for
  decoding the pinned-CA DER in `TlsSpec`). No API change at our call
  sites — `Engine`, `engine::general_purpose::STANDARD` and `DecodeError`
  are unchanged.
- The `rcgen` dev-dependency now builds with `default-features = false,
  features = ["crypto", "ring"]`. The harness only ever needs DER
  (`cert.der()`, `signing_key.serialize_der()`); rcgen's default `pem`
  feature pulls `pem -> base64 0.22`, which would collide with the new
  `base64 0.23` under `multiple-versions = "deny"`.
- Two `if !flag { if let Some(x) = … }` blocks in `src/query.rs`
  (`StreamState::drop`, `Rows::drop`) collapsed into let-chains. With
  `rust-version = "1.88"` declared, `clippy::collapsible_if` now knows
  let-chains are available and flags the nested form; the `clippy` CI job
  runs `-D warnings`. Behaviour is identical.

### Removed

- **The `rcgen` version cap.** `rcgen = ">=0.14.1, <0.14.8"` is back to
  plain `rcgen = "0.14"`. The cap existed only because rcgen 0.14.8+
  declares `rust-version = "1.88"`.
- **The `RUSTSEC-2026-0009` advisory ignore** — removed from
  `deny.toml`'s `[advisories]` and from the `ignore:` input on both the
  `audit` job in `ci.yml` and the daily `audit-cron.yml`. The ignore
  existed only because the fix, `time` 0.3.47, declares
  `rust-version = "1.88"`. With the MSRV at 1.88 a fresh resolve picks
  `time` 0.3.55 and `cargo deny check advisories` passes with
  `ignore = []`.
- **The workflow-level `RUSTFLAGS: "-D warnings"` in `ci.yml`.** It
  applied to every dependency as well as this crate, which busts the
  shared build cache (`RUSTFLAGS` is part of the fingerprint) and turns
  upstream warnings we do not control into build failures. Lint
  enforcement is unchanged in substance and now lives only where it
  belongs: the `clippy` job's `-- -D warnings`, the `docs` job's
  `RUSTDOCFLAGS`, and the `[lints]` table in `Cargo.toml`.

### Known issues

- `cargo deny check bans` still fails with a duplicate `syn` (2.0.119 vs
  3.0.4) under `--all-features`. This is pre-existing — it fails on
  `main` and on `0.4.1` too — and the 0.5.0 dependency moves do not
  close it: the syn-2 holdouts are `openssl-macros` (via `native-tls`),
  `tracing-attributes` and `zeroize_derive`, none of which we can move.
  Deliberately **not** papered over with a `[bans] skip`; the strict
  policy is intentional.

## [0.4.1] — 2026-08-25

A dependency-hygiene patch. No API changes.

### Fixed

- **CI clippy break on Rust 1.98.** `tests/drop_rows.rs` tripped the
  `clippy::useless_borrows_in_formatting` lint (`&*guard` in a `panic!`
  argument), which `make lint` / the `clippy` CI job run as
  `-D warnings`. Pre-existing code, newly linted by the current stable
  toolchain.
- **`rcgen` dev-dependency resolved above the declared MSRV.** `rcgen`
  `0.14.8`+ declares `rust-version = "1.88"` while this crate declares
  `1.85`, so a fresh resolve would break any MSRV job. The dev-dependency
  is now capped at `>=0.14.1, <0.14.8` with a comment pointing at the
  reason; lift the cap when the MSRV moves. This is the same 1.88 tension
  already documented in the `deny.toml` RUSTSEC-2026-0009 ignore.

### Changed

- Raised the TLS-stack dependency floors to current: `rustls`
  `0.23.18` -> `0.23.43`, `tokio-rustls` `0.26` -> `0.26.4`,
  `rustls-pki-types` `1` -> `1.15.1`, `webpki-roots` `1` -> `1.0.9`.
  There is no advisory between `0.23.18` (the RUSTSEC-2024-0399 patch
  level) and `0.23.43` — this is hardening against minimal-version
  resolution and stale downstream lockfiles, not a vulnerability fix.
  The `rustls` and `tokio-rustls` dev-dependency entries were moved in
  lockstep so `cargo-deny`'s `multiple-versions = "deny"` stays clean.
- The `tokio = "1.23.1"` floor is deliberately left alone; nothing in the
  crate needs a newer API.

## [0.4.0] — 2026-05-04

The observability + cleanup milestone. v0.4 layers opt-in `tracing` and
`metrics` over the v0.3 pool, tightens the `Reserved::rollback_on_drop`
contract, replaces the v0.3 real-network recycle fragility with a
registry-backed fast path, and finishes a long backlog of polish
items (idle-timeout enforcement, README as crate doctest, coverage CI,
typed transaction helpers).

### Added

#### Observability (opt-in, feature-gated)

- `tracing` feature — adds [`tracing`](https://docs.rs/tracing) span
  instrumentation on every public dispatch entry point: `Job::execute`,
  `Job::execute_with`, `Pool::execute`, `Pool::execute_with`,
  `Pool::acquire`, `Reserved::execute`, `Reserved::execute_with`. Spans
  carry `sql`, `param_count`, `tier` (for `Pool::execute*` — one of
  `try_idle`, `least_busy_scan`, `fair_queue`), and `Reserved` Drop
  emits a `trace` event with `rolled_back` and `in_tx` fields. Zero
  overhead when the feature is disabled.
- `metrics` feature — emits counters / gauges / histograms via the
  [`metrics`](https://docs.rs/metrics) facade. Metric-name constants live
  in the new `mapepire::observability` module and are SemVer-stable.
  Catalogue: `POOL_CREATE_TOTAL`, `POOL_RECYCLE_SUCCESS_TOTAL`,
  `POOL_RECYCLE_FAIL_TOTAL`, `POOL_ACQUIRE_LATENCY_MICROS`,
  `JOB_EXECUTE_LATENCY_MICROS`, `POOL_SIZE`, `POOL_AVAILABLE`,
  `POOL_WAITING`, `POOL_ROUTING_TIER_WINS_TOTAL` (1 label `tier`),
  `POOL_RESERVED_ACQUIRED_TOTAL`, `POOL_RESERVED_ROLLBACK_TOTAL`,
  `POOL_IDLE_REAPED_TOTAL`. Both features are zero-cost when disabled.
- Per-pool `ParameterLogging::{None, TypesAndCount, Full}` — stored on
  `PoolBuilder` since v0.3 but only enforced in v0.4. `Pool::execute_with`
  consults the policy and decorates its `tracing` span accordingly. The
  `None` default is privacy-safe (only `param_count` appears on spans).

#### Pool reliability

- `idle_timeout` enforcement (PRO-593). v0.3 stored the value on
  `PoolBuilder` but did not act on it. v0.4 spawns a background reaper
  task on `PoolBuilder::build` that wakes every `idle_timeout / 4`
  (clamped to `[1s, 60s]`) and calls `deadpool::Pool::retain` with the
  predicate `metrics.last_used() < idle_timeout`. The reaper is
  abort-on-last-`Pool`-clone-drop via an internal `ReaperGuard` so it
  doesn't outlive the pool.
- Registry-backed fast path in `Pool::execute` / `Pool::execute_with`
  (PRO-600). v0.3's step 1 used
  `deadpool::Pool::timeout_get(Timeouts { recycle: ZERO, .. })` which
  is `tokio::time::timeout(ZERO, ping)` — only ~1 timer-tick of grace.
  On real IBM i deployments where ping RTT exceeds that, the recycle
  ping timed out, deadpool detached the connection, and step 3 opened
  a fresh socket (connection thrash). v0.4 replaces the path with
  `Registry::peek_idle()` which filters `Weak<Job>` for `in_flight == 0`
  AND `Arc::strong_count == 2` (deadpool's slot + our upgrade — i.e.
  nobody has it checked out). The fast path skips deadpool's checkout
  entirely, so no recycle ping fires; liveness is verified at next
  dispatch.

#### Transactions

- `Reserved::begin`, `Reserved::commit`, `Reserved::rollback` typed
  helpers (PRO-599). Pure delegation to `Reserved::execute(...)` so the
  v0.4 transaction-state machine still observes every transition. Closes
  the stringly-typed `conn.execute("BEGIN")` ergonomics gap.
- `Reserved::execute` and `Reserved::execute_with` now observe the SQL
  prefix and update an internal `TxState` machine (`NotStarted` →
  `Started` on `BEGIN`; `Closed` on `COMMIT`/`ROLLBACK`). Used by Drop
  to gate the `rollback_on_drop` fire — see the **Changed** section.

#### DX & infrastructure

- README is now a crate-level doctest via
  `#![doc = include_str!("../README.md")]` (PRO-603). CI's
  `cargo doc` and `cargo test --doc` compile-check every Rust block
  in the README on every PR — API drift surfaces at PR time, not at
  user-bug time.
- `.github/workflows/coverage.yml` (PRO-606) — `cargo-llvm-cov` job
  uploading lcov to Codecov on push to `main` and on PRs. Closes the
  v0.2-era backlog item PRO-393.
- `make update-toolchain` (PRO-607) — reports MSRV / pinned channel /
  latest stable side-by-side so the maintainer can decide whether a
  bump is warranted. Closes the v0.2-era backlog item PRO-396.

### Changed

- **`Reserved::rollback_on_drop` (BEHAVIOR CHANGE).** v0.3's contract
  was unconditional once opt-in: Drop fired `ROLLBACK` even after an
  explicit `COMMIT`. Db2 returned `SQLSTATE 25000` and the pool tolerated
  it, but the round-trip wasted a wire turn. v0.4 tightens the contract:
  Drop fires `ROLLBACK` only when both (a) `rollback_on_drop` is set and
  (b) a `BEGIN` has been observed without a matching `COMMIT`/`ROLLBACK`.
  Suppresses redundant rollbacks after explicit `COMMIT` and on
  connections that never began a transaction.
- `Pool::execute` step 1 — see **Added → Pool reliability**. The
  observability contract is unchanged (same `try_idle` tier label).
- `pool::pool` module renamed to `pool::runtime` (PRO-605). Public API
  identical (`crate::Pool`, `crate::PoolStatus` re-exports unchanged);
  the inner module name was an implementation detail. Retires the v0.3
  `#[allow(clippy::module_inception)]`.
- Tightened terse `expect()` messages in `src/` (PRO-608). Sites in
  `lib.rs`, `config.rs`, `from_row.rs`, and `pool/builder.rs` now name
  what was expected to succeed instead of one-word shorthand.

### Removed

- `#[allow(clippy::module_inception)]` on `pool::pool` (subsumed by the
  rename to `pool::runtime`).
- `Pool::execute`'s v0.4-caveat comment block describing the
  `recycle: ZERO` fragility — the fragility is gone.

### Testing

- `tests/pool_recycle_latency.rs::fast_path_does_not_thrash_under_slow_response`
  (PRO-601). Pins the registry-backed fast path against the v0.3
  fragility — single-slot pool, `mock.pause_responses(100ms)` across
  two `pool.execute` calls, asserts `open_socket_count == 1` and zero
  ping requests fire. Would have failed under the v0.3 contract.
- `tests/pool_idle_timeout.rs` (PRO-594). Builds a pool with
  `idle_timeout(Some(500ms))`, opens 1 connection, waits past the reap
  window, asserts the connection was reaped and a subsequent execute
  opens a fresh socket.
- `MockHandle::wait_for_sql(needle, timeout)` test primitive (PRO-604)
  — polls `last_socket_for_sql` every 10ms with a configurable budget.
  Replaces a fragile `sleep(100ms)` in the positive ROLLBACK test.
- Integration coverage for `tracing` and `metrics` features
  (`tests/tracing_spans.rs`, `tests/metrics_smoke.rs`).

### Internal

- New `mapepire::observability` module exposing the metric-name constants.
- `Registry::peek_idle()` — `Arc<Job>` lookup with the `strong_count == 2`
  liveness predicate (PRO-600).
- `Reserved` carries a `Mutex<TxState>` (chosen over `Cell` because
  `Reserved: Sync`).
- `PoolBuilder::build` spawns the idle reaper and returns a `ReaperGuard`
  inside the `Pool` so the task aborts on the last `Pool` clone drop.

## [0.3.0] — 2026-05-04

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

### Open items deferred to v0.4+

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

[Unreleased]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.7.2...HEAD
[0.7.2]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/MeridianGroupInt/mapepire-rs/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.4.0
[0.3.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.3.0
[0.2.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.2.0
[0.1.0]: https://github.com/MeridianGroupInt/mapepire-rs/releases/tag/v0.1.0
