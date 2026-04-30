# AGENTS.md — `mapepire` contributor & AI-assistant guide

This file is the single source of truth for how work happens in this repo.
It applies equally to human contributors and to AI assistants (Claude,
Copilot, Cursor, etc.). Where it conflicts with default tool behavior,
this guide wins.

> Open-source convention: tools that look for `AGENTS.md`, `CLAUDE.md`, or
> `CONTRIBUTING.md` should all be pointed here. Symlinks are fine; the
> content is canonical in `AGENTS.md`.

---

## 1. What this crate is

`mapepire` is the Rust client SDK for [Mapepire](https://mapepire-ibmi.github.io/) —
IBM's open-source database access layer for **Db2 for IBM i** that exposes
the database over TLS-secured WebSockets. Sibling SDKs exist for Node.js,
Python, Java, Go, PHP, C#/.NET, and RPG; this crate fills the Rust gap.

**Quality bar:** donation-ready. The eventual goal is to propose this
crate to the [`Mapepire-IBMi`](https://github.com/Mapepire-IBMi) GitHub
org as the official Rust SDK. Architecture, documentation, security
posture, and CI shape are all calibrated to that bar.

**Status:** pre-1.0. v0.1 is the protocol foundation (types only, no
network). v0.2 adds transport. v0.3 adds the connection pool. v1.0 is the
release milestone.

---

## 2. Repository layout (target)

```
mapepire-rs/
├── Cargo.toml              # crate manifest — single crate, not a workspace
├── Makefile                # canonical task runner
├── AGENTS.md               # ← you are here
├── SECURITY.md             # vulnerability reporting
├── README.md
├── CHANGELOG.md            # Keep-a-Changelog format
├── LICENSE-MIT
├── LICENSE-APACHE
├── .rustfmt.toml           # nightly rustfmt config
├── clippy.toml             # clippy thresholds
├── deny.toml               # cargo-deny supply-chain policy
├── .editorconfig
├── .github/
│   ├── PULL_REQUEST_TEMPLATE.md
│   ├── dependabot.yml
│   └── workflows/          # CI (added in v0.1 Task 2)
├── src/
│   ├── lib.rs              # crate-level docs + re-exports
│   ├── error.rs            # Error / Result / sub-error types
│   ├── config.rs           # DaemonServer + builder + TlsConfig
│   ├── password.rs         # Password (zeroize, no-Clone)
│   └── protocol/
│       ├── mod.rs
│       ├── request.rs
│       ├── response.rs
│       └── codec.rs
├── tests/                  # integration + snapshot + proptest harnesses
└── examples/               # added at v0.2+
```

**Plan and spec documents live outside this repo** (in the maintainer's
research workspace). They are not committed here. Don't copy them in.

---

## 3. Module dependency rules

Strict layering — a module may only depend on modules below it:

```
protocol  ─┐
error     ─┼──► (no inter-dependencies between these three)
password  ─┘
              │
              ▼
            config         (uses password)
              │
              ▼
            transport      (v0.2 — uses error, protocol)
              │
              ▼
            job            (v0.2 — uses transport, protocol, config)
              │
              ▼
            pool           (v0.3 — uses job, error)
```

`lib.rs` is the public-API hub: every consumer-visible type is `pub use`'d
from there. Internals stay `pub(crate)` or private.

When in doubt, **smaller files** beat large ones. Each file should have
one clear responsibility you can describe in a sentence.

---

## 4. Build, test, and format

The `Makefile` is the canonical interface — CI runs the same targets, so
a green local `make pre-pr` is a strong signal the PR will pass CI.

### One-time setup
```sh
make setup    # installs cargo-audit, cargo-deny, cargo-llvm-cov, cargo-outdated, cargo-fuzz, nightly toolchain
```

### Daily commands
```sh
make build          # cargo build --all-features
make test           # cargo test --all-features
make lint           # clippy with -D warnings
make format         # cargo +nightly fmt --all
make pre-commit     # format + fix (run before each commit)
```

### Before opening a PR
```sh
make pre-pr         # format-check + lint + test + audit + deny + doc
```

This is the same set of checks CI runs. If `pre-pr` is clean, CI is
extremely likely to be clean. **Don't skip it.**

### Other useful targets
```sh
make coverage       # HTML report at target/llvm-cov/html/index.html
make outdated       # check for newer dependency versions
make doc-open       # build docs and open in browser
make msrv-check     # verify the crate still builds on the declared MSRV
make release-check  # full pre-pr + cargo publish --dry-run
```

`make help` lists everything.

### Formatting

We use **nightly rustfmt** because several formatting options we want
(`wrap_comments`, `format_code_in_doc_comments`, `imports_granularity`)
are nightly-only. The formatter still runs against stable code; only the
rustfmt binary is nightly. `make setup` installs the toolchain.

If you don't have nightly available, `cargo fmt --all` (stable) will work
but won't apply the comment-formatting options. CI runs the nightly path.

---

## 5. Coding standards

### 5.1 Module organization

- Every module has a module-level `//!` doc comment explaining its role.
- Every `pub` item has a `///` doc comment with at least one example
  where the example would compile (use `no_run` for network-bound code).
- `missing_docs = "deny"` is a crate-level lint. Don't relax it.
- `lib.rs` re-exports the public API so consumers `use mapepire::Foo`,
  not `use mapepire::config::Foo`.

### 5.2 Error handling

- Single crate-wide `Error` enum in `src/error.rs`. Public API returns
  `mapepire::Result<T>` — alias for `std::result::Result<T, Error>`.
- `Error` is `#[non_exhaustive]`. Adding a variant is a minor-version
  bump.
- Wrapper structs (`TransportError`, `ProtocolError`, `DecodeError`,
  `ServerError`) so we can extend them with new fields without breaking
  semver.
- `From` impls for upstream error types are **private** to the `error`
  module — we control what crosses the boundary.
- Library code must not panic. **Never** call `unwrap`/`expect` outside
  test code or in `Drop` impls where panicking would abort. If an
  invariant is genuinely unreachable, return `Error::Internal(...)` with
  a descriptive message.

### 5.3 Async/await (lands in v0.2)

- Tokio-only. The crate depends on tokio with the features it needs;
  consumers do not need to opt in to anything.
- Don't block the runtime. No `std::thread::sleep`, no synchronous I/O.
  CPU-heavy work goes through `tokio::task::spawn_blocking`.
- Cancellation safety is non-negotiable. Dropping any future returned by
  the public API must not leak resources or leave the connection in an
  invalid state. The dispatcher pattern is built around this guarantee.

### 5.4 Testing

- **Unit tests** in `#[cfg(test)] mod tests` blocks at the bottom of the
  file under test.
- **Integration tests** in the top-level `tests/` directory.
- **Snapshot tests** in `tests/wire_snapshots.rs` using `insta`. First
  run with `INSTA_UPDATE=auto cargo test --test wire_snapshots`, then
  **manually inspect** every generated `.snap` file before committing.
  Snapshots that aren't reviewed defeat their purpose.
- **Property-based tests** in `tests/proptest_round_trips.rs`. Minimum
  256 cases per property. Add new properties when adding new wire types.
- **Doctests** for every public item that can run without a network. Use
  `no_run` for network-bound examples — they compile, which is the
  guarantee we actually want.
- **Test naming**: `test_<function>_<scenario>` for unit tests; the
  scenario describes what's being verified.
- **Arrange / Act / Assert** structure within each test. Don't conflate.
- Use `pretty_assertions::assert_eq!` for any non-trivial comparison —
  the diffs are dramatically easier to read than std's.

### 5.5 Logging (lands in v0.4)

- `tracing` only. No `println!`, no `log::` macros, no `eprintln!`
  outside tests.
- Structured fields, not string interpolation:
  ```rust
  tracing::info!(job_name = %name, rows = count, "executed query");
  ```
- Span shape follows OpenTelemetry semantic conventions for database
  client calls (`db.system="db2i"`, `db.statement`, `db.rows`,
  `db.rows_affected`, `db.duration_ms`).
- See `SECURITY.md` for redaction rules. Default: SQL truncated, **no**
  parameter values logged.

### 5.6 Configuration

- `DaemonServer` is **not** `Clone` (because `Password` isn't). Wrap in
  `Arc<DaemonServer>` to share across pools. `Pool::builder` accepts
  `impl Into<Arc<DaemonServer>>` so either path is ergonomic.
- File-based config goes through `DaemonServerSpec` (DTO with serde
  derives), gated by the `serde-config` feature.

### 5.7 Commit format

Conventional commits, imperative mood, subject < 72 characters:

```
<type>(<scope>): <subject>

<body explaining why, not what>

Co-Authored-By: <name> <email>
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `ci`, `deps`.

When AI assistants contribute, the `Co-Authored-By` line credits them —
e.g., `Co-Authored-By: Claude <noreply@anthropic.com>`.

---

## 6. Security invariants (load-bearing — see SECURITY.md)

These must always hold:

1. **`Password`** is never `Clone`, `Serialize`, `Deserialize`,
   `Display`, `PartialEq`, or `Hash`. Buffer is zeroized on drop. The
   `zeroize_clears_buffer` test in `src/password.rs` is the regression
   guard — it uses `ManuallyDrop` to suspend deallocation, calls
   `Zeroize::zeroize()` on the inner buffer, and asserts the bytes are
   zero while the allocation is still live. Do not delete or weaken it.
2. **TLS is mandatory.** No plaintext path. The `TlsConfig::Insecure`
   variant requires the `insecure-tls` feature flag at compile time and
   emits a runtime warning the first time it's used.
3. **`IdAllocator`** produces collision-free correlation ids across
   `Job` instances and after counter wraparound. Modifying its
   construction requires a regression test that exercises both
   conditions.
4. **Parameter logging** defaults to `ParameterLogging::None`. The
   default never changes silently — promoting `TypesAndCount` or `Full`
   to default is a breaking change and a security review event.

Regressions on any of these are P0.

---

## 7. Feature flag conventions

- `default = ["rustls-tls"]`. TLS is the only default-on feature because
  the crate cannot function without one.
- All other features are off by default; consumers opt in.
- `compile_error!` if neither `rustls-tls` nor `native-tls` is enabled —
  silent build failure is worse than an opinionated default. Already in
  `lib.rs`.
- Feature-gated public items use `#[cfg_attr(docsrs, doc(cfg(...)))]`
  so docs.rs renders them with badges.
- Don't add a feature without a documented reason it shouldn't be
  default-on.

---

## 8. Wire protocol invariants

- `Request` and `Response` enums are `#[serde(tag = "type",
  rename_all = "snake_case")]`. Every variant maps 1:1 to a Mapepire
  protocol operation. Don't add variants that aren't on the wire.
- Both enums are `#[non_exhaustive]`.
- Optional fields on the wire use `#[serde(skip_serializing_if =
  "Option::is_none")]` so we don't emit `null` for absent values.
- New wire variants require a `tests/wire_snapshots.rs` entry. The
  reviewer reads the generated `.snap` to verify the shape matches what
  the daemon actually expects.

---

## 9. MSRV policy

- MSRV = current Rust stable **minus 2 minor releases**. Set in
  `Cargo.toml [package] rust-version`.
- Bumping MSRV is a **minor version bump** for the crate, called out in
  the changelog under "Changed".
- A dedicated CI job (`msrv`) checks the crate compiles at the declared
  MSRV. Don't skip it.
- We don't go below the declared MSRV for any reason.

---

## 10. Dual-license posture

The crate is licensed `MIT OR Apache-2.0`. Both `LICENSE-MIT` and
`LICENSE-APACHE` live at the repository root. Consumers pick whichever
license fits their project.

**All contributions are accepted under both licenses.** By submitting a
pull request, you agree to license your contribution under both MIT and
Apache-2.0.

The Apache side carries an explicit patent grant and aligns with the
sibling Mapepire SDKs (which are Apache-2.0). The MIT side keeps us
compatible with downstream Rust consumers that prefer the simpler text.

---

## 11. Donation-readiness checklist

Updated as we approach v1.0; tracked in the Linear project alongside the
v1.0 milestone.

- [ ] All ~15 sibling-SDK protocol operations covered (v0.1)
- [ ] Transport, connection, pool, transactions (v0.3)
- [ ] Observability features (v0.4)
- [ ] Examples covering the same use cases as `mapepire-js` (v1.0)
- [ ] At least one real-IBM-i nightly CI run green (v1.0)
- [ ] RustDoc surface complete; docs.rs renders all features
- [ ] No GPL deps. Mapepire's *server* is GPL-3.0; an Apache-2.0 client
      is the right shape for sibling parity
- [ ] Public benchmark vs. an ODBC alternative (post-1.0)
- [ ] Maintainer outreach issue filed at
      [`Mapepire-IBMi/.github`](https://github.com/Mapepire-IBMi)

---

## 12. References

- **Mapepire docs**: https://mapepire-ibmi.github.io/
- **Mapepire-IBMi org**: https://github.com/Mapepire-IBMi
- **Mapepire wire protocol**:
  https://github.com/Mapepire-IBMi/mapepire-protocol
- **Sibling SDKs** for cross-checking behavior:
  - mapepire-js (TypeScript)
  - mapepire-python
  - mapepire-java
  - mapepire-go
  - mapepire-php
  - mapepire-csharp
- **Linear project**: see the maintainer's Linear workspace under
  Product → Mapepire Rust Crate
- **Design spec & implementation plans**: maintained outside this repo
  in the maintainer's research workspace. Maintainers can share
  excerpts when they're relevant to a PR review.

---

## 13. Working with AI assistants

This guide is the briefing material. AI assistants invoked in this repo
should read it before proposing changes; agents in this codebase are
expected to:

- Run `make pre-pr` before claiming a task is complete. Verification
  precedes assertion.
- Match existing code patterns rather than introduce new ones.
- Keep changes scoped to the task. No drive-by refactors.
- Surface concerns explicitly. "Done with concerns" beats silently
  shipping something the maintainer wouldn't have chosen.
- Use the PR template; fill in every section.
- Default to small files and small functions. If a file goes over ~500
  lines, suggest splitting it.

The donation goal is the north star. When a tradeoff is unclear, pick
the option that makes the crate easier for the Mapepire-IBMi org to
adopt.
