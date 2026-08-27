# Security policy

`mapepire` handles credentials and database connections. Please report
security issues privately, **not** as public GitHub issues — public reports
of an unpatched vulnerability put every consumer of the crate at risk.

## Reporting a vulnerability

**Preferred:** Use GitHub's [private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing/privately-reporting-a-security-vulnerability)
on this repository ("Security" tab → "Report a vulnerability"). That opens a
private discussion with maintainers and lets us coordinate a fix and
disclosure.

**Email fallback:** dan.curtis@meridianitinc.com

We aim to acknowledge reports within **72 hours** and provide a triage
update within **one week**. Coordinated disclosure is the default; the
reporter gets credit unless they prefer otherwise.

## Supported versions

Pre-1.0 releases: only the latest minor receives security fixes. Upgrade
promptly when a new minor lands.

Post-1.0: the latest minor in each major line is supported.

## Security-relevant invariants

These properties are load-bearing for the crate's threat model. Reports of
regressions are treated as **P0**:

- `Password` is never `Clone`, `Serialize`, `Deserialize`, `Display`,
  `PartialEq`, or `Hash`. Its inner buffer is zeroized on drop.
- **Wire-protocol boundary:** `Request::Connect` does **not** carry
  credentials. Live daemon authentication is HTTP Basic on the WebSocket
  upgrade. `Password::expose() -> &str` is used to build the
  `Authorization: Basic` header. The `user:pass` concatenation is a
  `Zeroizing<String>` and is dropped after Base64 encoding. The header
  value is not logged. The password is not on the query string and is
  not in connect JSON. The `Password` itself remains zeroize-on-drop.
- TLS is mandatory. There is no plaintext path to the daemon. The
  `TlsConfig::Insecure` variant must be opted into via the `insecure-tls`
  Cargo feature at compile time and emits a runtime warning when used.
- Default TLS verification uses webpki roots (`TlsConfig::Verified`) and
  never skips name checks. On rustls, `TlsConfig::Ca` treats an exact
  leaf-DER match as a pin (SAN skipped); a non-matching leaf still uses
  `WebPkiServerVerifier` with name checks. native-tls is unchanged
  (OpenSSL CN fallback). `cargo deny check` enforces the supply-chain
  policy on every PR.
- `IdAllocator` produces collision-free correlation ids across `Job`
  instances. Modifying its construction requires a regression test.
- Parameter logging defaults to `ParameterLogging::None` and never logs
  values without an explicit opt-in. Changing the default is a breaking
  change and requires a security review.

## Yank policy

Confirmed CVEs in the crate or in any unpatchable transitive dependency
trigger a `cargo yank` of the affected versions within 24 hours, followed
by a fixed release as soon as a patch is ready.
