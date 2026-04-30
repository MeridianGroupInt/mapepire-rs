# mapepire

[![CI](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/ci.yml?query=branch%3Amain)
[![Audit (daily)](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/audit-cron.yml/badge.svg)](https://github.com/MeridianGroupInt/mapepire-rs/actions/workflows/audit-cron.yml)
[![deps.rs](https://deps.rs/repo/github/MeridianGroupInt/mapepire-rs/status.svg)](https://deps.rs/repo/github/MeridianGroupInt/mapepire-rs)
[![MSRV](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2FMeridianGroupInt%2Fmapepire-rs%2Fmain%2FCargo.toml&query=%24.package.rust-version&label=MSRV&color=blue)](Cargo.toml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)

<!-- Uncomment after first crates.io publish:
[![crates.io](https://img.shields.io/crates/v/mapepire.svg)](https://crates.io/crates/mapepire)
[![docs.rs](https://img.shields.io/docsrs/mapepire)](https://docs.rs/mapepire)
-->

Async Rust client SDK for [Mapepire](https://mapepire-ibmi.github.io/) —
a cloud-friendly access layer for **Db2 for IBM i** that exposes the
database over TLS-secured WebSockets.

> **Status:** v0.1 in progress (protocol foundation). Not yet on
> [crates.io](https://crates.io). The full v1.0 surface (transport,
> connection, pool, observability, examples) lands across the
> v0.1 → v1.0 milestones.

Sibling SDKs exist for [Node.js](https://github.com/Mapepire-IBMi/mapepire-js),
[Python](https://github.com/Mapepire-IBMi/mapepire-python),
[Java](https://github.com/Mapepire-IBMi/mapepire-java),
[Go](https://github.com/Mapepire-IBMi/mapepire-go),
[PHP](https://github.com/Mapepire-IBMi/mapepire-php), and
[C#/.NET](https://github.com/Mapepire-IBMi/mapepire-csharp). This crate
fills the Rust gap with a parity-first design.

## Documentation

- [`AGENTS.md`](AGENTS.md) — contributor and AI-assistant guide
  (architecture, coding standards, security invariants, MSRV policy)
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to open a PR
- [`SECURITY.md`](SECURITY.md) — vulnerability reporting
- [`Makefile`](Makefile) — `make help` lists all dev tasks

## License

Dual-licensed under either of:

- [MIT license](LICENSE-MIT) ([https://opensource.org/licenses/MIT](https://opensource.org/licenses/MIT))
- [Apache License, Version 2.0](LICENSE-APACHE) ([https://www.apache.org/licenses/LICENSE-2.0](https://www.apache.org/licenses/LICENSE-2.0))

at your option. By contributing, you agree your contribution will be
dual-licensed as above.
