# mapepire

[![CI](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/ci.yml?query=branch%3Amain)
[![Audit (daily)](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/audit-cron.yml/badge.svg)](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/audit-cron.yml)
[![deps.rs](https://deps.rs/repo/github/MeridianGroupInt/mapepire-rs/status.svg)](https://deps.rs/repo/github/MeridianGroupInt/mapepire-rs)
[![MSRV](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2FMeridianGroupInt%2Fmapepire-rs%2Fmain%2FCargo.toml&query=%24.package.rust-version&label=MSRV&color=blue)](Cargo.toml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![crates.io](https://img.shields.io/crates/v/mapepire.svg)](https://crates.io/crates/mapepire)
[![docs.rs](https://img.shields.io/docsrs/mapepire)](https://docs.rs/mapepire)

Async Rust client SDK for [Mapepire](https://mapepire-ibmi.github.io/) —
a cloud-friendly access layer for **Db2 for IBM i** that exposes the
database over TLS-secured `WebSockets`.

> **Status:** v0.7 speaks a live Mapepire daemon (mapepire-js 0.6.x
> handshake, CL, bind, terse, CALL/OUT, trace dest, sqlmore paging,
> gettracedata, dove, and one-shot parameter sets). On
> [crates.io](https://crates.io/crates/mapepire). Real-IBM-i CI lands in
> v1.0.

Sibling SDKs exist for [Node.js](https://github.com/Mapepire-IBMi/mapepire-js),
[Python](https://github.com/Mapepire-IBMi/mapepire-python),
[Java](https://github.com/Mapepire-IBMi/mapepire-java),
[Go](https://github.com/Mapepire-IBMi/mapepire-go),
[PHP](https://github.com/Mapepire-IBMi/mapepire-php), and
[C#/.NET](https://github.com/Mapepire-IBMi/mapepire-csharp). This crate
fills the Rust gap with a parity-first design.

## Quick look

```rust,no_run
use mapepire::{DaemonServer, Pool, TlsConfig};

# async fn example() -> mapepire::Result<()> {
let server = DaemonServer::builder()
    .host("ibmi.example")
    .user("USER")
    .password(std::env::var("MAPEPIRE_PASSWORD").unwrap())
    .tls(TlsConfig::Verified)
    .build()
    .expect("missing required field");

let pool = Pool::builder(server).max_size(8).build().await?;

// One-shot SQL via the routed pool.
let _rows = pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1").await?;

// Transactional work via Reserved (BEGIN / DML / COMMIT all on one socket).
let conn = pool.acquire().await?.rollback_on_drop();
conn.execute("BEGIN").await?;
conn.execute_with(
    "UPDATE ORDERS SET STATUS = ? WHERE ID = ?",
    &[serde_json::json!("paid"), serde_json::json!(42)],
).await?;
conn.execute("COMMIT").await?;
# Ok(()) }
```

`Pool::builder(server).max_size(8).build().await?` warms a `deadpool`-backed
connection pool of `Job` handles. `pool.execute(...)` runs one-shot SQL via
the v0.3 §7.3 three-tier routing scan (idle → least-busy → fair-queue).
`pool.acquire()` returns a [`Reserved`] handle that pins one socket for
`BEGIN` / DML / `COMMIT` so the entire transaction lands on the same
connection. The opt-in `.rollback_on_drop()` fires a best-effort `ROLLBACK`
if the handle drops without an explicit `COMMIT` / `ROLLBACK`.

The `password` setter takes ownership of a `String` and immediately moves
it into a zeroizing buffer (`Password`). `DaemonServer` is not `Clone` —
the `Pool::builder` constructor takes `impl Into<Arc<DaemonServer>>` so the
single config is shared across every pooled connection.

### Self-signed IBM i (CN-only) and SSH tunnels

Stock IBM i Mapepire certs are often CN-only (no SAN). Pin the leaf DER
with [`TlsConfig::Ca`] — the default `rustls-tls` backend skips SAN/CN
checks on an exact DER match. If TCP is not the certificate name (laptop
SSH forward), set `connect_address` separately; `host` stays SNI / HTTP
`Host` / the cert name.

```rust,no_run
use mapepire::{DaemonServer, TlsConfig};

# async fn example() -> mapepire::Result<()> {
let der = std::fs::read("daemon.der").expect("leaf DER from out-of-band pin");
let server = DaemonServer::builder()
    .host("ibmi.example")
    .connect_address("127.0.0.1")
    .user("USER")
    .password(std::env::var("MAPEPIRE_PASSWORD").unwrap())
    .tls(TlsConfig::Ca(der))
    .build()
    .expect("missing required field");
# let _ = server;
# Ok(()) }
```

| Situation | What to use |
|---|---|
| Public / webpki-trusted daemon cert | default `rustls-tls` + [`TlsConfig::Verified`] |
| IBM i self-signed, CN-only (no SAN) | default `rustls-tls` + [`TlsConfig::Ca`] (leaf pin). Alternative: `default-features = false, features = ["native-tls"]` (OpenSSL CN fallback). **Never** `insecure-tls` for this. |
| SSH tunnel (`host` ≠ TCP hop) | `host` = certificate / SNI name (`ibmi.example`); `connect_address` = TCP (`127.0.0.1`) |

Obtain the pin bytes out-of-band (admin-provided DER, or `openssl s_client`
plus a fingerprint check). Do not skip verification in production.

### Runnable examples

The [`examples/`](examples/) directory ships six runnable demos covering
the common patterns:

| Example | Demonstrates |
|---|---|
| [`examples/one_shot.rs`](examples/one_shot.rs) | `Pool::builder` → `pool.execute(sql)` → iterate rows |
| [`examples/prepared.rs`](examples/prepared.rs) | `Job::prepare` + `Query::execute_with` reused across calls |
| [`examples/transaction.rs`](examples/transaction.rs) | `pool.acquire().rollback_on_drop()` + v0.4 typed `begin/commit` |
| [`examples/streaming.rs`](examples/streaming.rs) | `Rows::stream_typed::<T>` with a `serde::Deserialize` row struct |
| [`examples/with_tracing.rs`](examples/with_tracing.rs) | `tracing-subscriber` registration + per-execute span output |
| [`examples/cl_command.rs`](examples/cl_command.rs) | `Job::cl(...)` + `ClOutcome` job-log walkthrough |

Each example reads `MAPEPIRE_HOST`, `MAPEPIRE_USER`, and `MAPEPIRE_PASSWORD`
from the environment. [`examples/one_shot.rs`](examples/one_shot.rs) also
honors `MAPEPIRE_CONNECT_ADDRESS` (TCP hop, e.g. `127.0.0.1` through an
SSH tunnel), `MAPEPIRE_PORT`, `MAPEPIRE_PROPS` (JDBC properties string),
and `MAPEPIRE_CA` (path to a DER leaf pin for [`TlsConfig::Ca`]). Run
with `cargo run --example <name>` (or
`cargo run --example with_tracing --features tracing` for the tracing demo).

### Single-connection alternative

If you only need one connection (e.g. a CLI tool or a one-shot script),
`Job::connect` skips the pool entirely:

```rust,no_run
use mapepire::{DaemonServer, Job, TlsConfig};

# async fn example() -> mapepire::Result<()> {
let server = DaemonServer::builder()
    .host("ibmi.example")
    .port(8076)
    .user("USER")
    .password(std::env::var("MAPEPIRE_PASSWORD").unwrap())
    .tls(TlsConfig::Verified)
    .build()
    .expect("missing required field");

let job = Job::connect(&server).await?;
let rows = job.execute("SELECT NAME, COUNT FROM SCHEMA.STATS").await?;
let dynamic = rows.into_dynamic().await?;
for row in dynamic {
    let name: String = row.get("NAME")?;
    let count: i64 = row.get("COUNT")?;
    println!("{name}: {count}");
}
# Ok(()) }
```

`Job::connect` performs the full TCP → TLS → WebSocket Upgrade →
`Connect` handshake and resolves once the daemon confirms the session.
[`Job::version`] may be empty on a live connect (the connect frame often
omits it); call [`Job::server_version`] (`getversion`) for the daemon
version string.

## Cargo features

| Feature | Default | Purpose |
|---|---|---|
| `rustls-tls` | **on** | Pure-Rust TLS via `rustls` with the `ring` `CryptoProvider`. [`TlsConfig::Ca`] is a leaf pin (SAN skipped on exact DER match). |
| `native-tls` | off | OS-platform TLS via `native-tls` (OpenSSL CN fallback for CN-only IBM i certs) |
| `insecure-tls` | off | Compile-time gate for `TlsConfig::Insecure` (skip server-cert validation; **never use in production** — CN-only IBM i certs use [`TlsConfig::Ca`], not this) |
| `serde-config` | off | `DaemonServerSpec` DTO for loading from config files (TOML/YAML/JSON via consumer's choice of parser) |
| `tracing` | off | [`tracing`](https://docs.rs/tracing) span instrumentation on every public dispatch entry point (`Job::execute`, `Pool::execute`, `Reserved::*`). Per-pool [`ParameterLogging`](https://docs.rs/mapepire/latest/mapepire/enum.ParameterLogging.html) governs whether parameter values appear on spans. |
| `metrics` | off | [`metrics`](https://docs.rs/metrics) facade integration. Counters / gauges / histograms documented at [`mapepire::observability`](https://docs.rs/mapepire/latest/mapepire/observability/index.html). |

A `compile_error!` guard fires when neither `rustls-tls` nor `native-tls`
is enabled — disabling default features requires an explicit alternate
TLS backend selection.

## Observability (optional)

Enable `tracing` and/or `metrics` features for production observability:

```toml
[dependencies]
mapepire = { version = "0.7", features = ["rustls-tls", "tracing", "metrics"] }
tracing-subscriber = "0.3"
metrics-exporter-prometheus = "0.15"
```

```rust,ignore
# fn install() -> Result<(), Box<dyn std::error::Error>> {
// Tracing — fmt subscriber to stderr.
tracing_subscriber::fmt::init();

// Metrics — Prometheus exporter on :9000/metrics.
metrics_exporter_prometheus::PrometheusBuilder::new().install()?;
# Ok(()) }
```

Once installed, every `Job::execute` / `Pool::execute` / `Pool::acquire` /
`Reserved::*` call emits the relevant spans and metrics. Both features are
zero-cost when disabled.

The metric-name contract is documented at
[`mapepire::observability`](https://docs.rs/mapepire/latest/mapepire/observability/index.html)
and is **SemVer-stable** — names won't be renamed without a major bump.

## Roadmap

- **v0.1** — protocol foundation (done).
- **v0.2** — transport, `Job::connect`, integration tests (done).
- **v0.3** — `Pool` with `deadpool`, `Reserved` for transactions, public
  `Executor` / `FromRow` traits, diagnostic methods carried over from
  v0.2 (done).
- **v0.4** — `tracing` and `metrics` feature flags; `idle_timeout`
  enforcement; `rollback_on_drop` tightened to only-if-in-tx;
  registry-backed routing fast path (done).
- **v0.5** — MSRV 1.88; `Request` Debug redacts credentials (done).
- **v0.6** — live Mapepire handshake: `/db/` + HTTP Basic, untagged
  responses, rustls `ring`, [`TlsConfig::Ca`] leaf pin, `connect_address`
  (done).
- **v0.7** — live dialect leftovers: `ClOutcome`, `rows: 100`, terse
  array rows, CALL/OUT `output_parms`, `FILE`/`IN_MEM` trace dest,
  sqlmore paging, gettracedata, dove `vedata`, one-shot `execute_sets`
  (done).
- **v1.0** — real-IBM-i CI.

## Documentation

- [`AGENTS.md`](AGENTS.md) — contributor and AI-assistant guide
  (architecture, coding standards, security invariants, MSRV policy)
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to open a PR
- [`SECURITY.md`](SECURITY.md) — vulnerability reporting
- `Makefile` — `make help` lists all dev tasks

## License

Dual-licensed under either of:

- [MIT license](LICENSE-MIT) ([https://opensource.org/licenses/MIT](https://opensource.org/licenses/MIT))
- [Apache License, Version 2.0](LICENSE-APACHE) ([https://www.apache.org/licenses/LICENSE-2.0](https://www.apache.org/licenses/LICENSE-2.0))

at your option. By contributing, you agree your contribution will be
dual-licensed as above.
