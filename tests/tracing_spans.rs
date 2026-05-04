//! Tracing-feature integration test: verify span emission and the `tier`
//! field on `Pool::execute`, plus the synchronous `Reserved` Drop trace
//! event.
//!
//! Per-item `#[cfg(all(feature = "rustls-tls", feature = "tracing"))]`
//! gating since the mock harness is rustls-only and the spans only fire
//! when tracing is enabled. Crate-level `#![cfg]` is intentionally avoided —
//! it would strip the `//!` doc comment and trip the crate-wide
//! `missing_docs = "deny"` lint.
//!
//! # Why a custom `Layer`, not `tracing-test`
//!
//! `tracing::instrument` (Tasks 7/8/9) opens a span but does not emit any
//! event on entry, and `Span::current().record("tier", ...)` likewise
//! emits nothing — it just updates the open span's stored attributes.
//! `tracing-test` 0.2 is built on the default `FmtSubscriber`, which only
//! writes events to its buffer (no span lifecycle output without the
//! `FmtSpan::*` builder switch, which `tracing-test` does not expose).
//! That means a `tracing-test`-backed assertion against `Pool::execute`
//! would observe an empty buffer.
//!
//! The custom `Layer` below captures BOTH paths: `on_new_span`
//! (records span name + creation-time fields) and `on_record`
//! (records follow-up `Span::record(...)` calls like the `tier` marker
//! that's filled in once routing decides which §7.3 tier handled the
//! request). Standard tracing-book pattern for span-shape assertions.

#[cfg(all(feature = "rustls-tls", feature = "tracing"))]
mod common;

#[cfg(all(feature = "rustls-tls", feature = "tracing"))]
mod capture {
    //! Tiny `tracing` layer that records span openings, span field
    //! updates, and events into an `Arc<Mutex<Vec<String>>>`.
    //!
    //! Each entry is a single line of the form:
    //! - `span: <name> <field=value> ...`     (from `on_new_span`)
    //! - `record: <name> <field=value> ...`   (from `on_record`)
    //! - `event: <message?> <field=value> ...`(from `on_event`)
    //!
    //! Uniform formatting keeps assertions trivial — tests just match on
    //! `name=value` substrings or message text.
    use std::fmt::{self, Write as _};
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::registry::LookupSpan;

    /// Shared buffer of formatted log lines. Cloning is cheap (Arc).
    pub type Buffer = Arc<Mutex<Vec<String>>>;

    /// Build a fresh buffer.
    pub fn new_buffer() -> Buffer {
        Arc::new(Mutex::new(Vec::new()))
    }

    /// Capture layer for the [`Buffer`].
    pub struct CaptureLayer {
        buf: Buffer,
    }

    impl CaptureLayer {
        pub fn new(buf: Buffer) -> Self {
            Self { buf }
        }

        fn push(&self, line: String) {
            self.buf
                .lock()
                .expect("capture buffer mutex poisoned")
                .push(line);
        }
    }

    /// Visitor that flattens fields into `key=value` pairs, separated by
    /// spaces. `message` is rendered without the `message=` prefix so
    /// log-message text reads naturally.
    struct StringVisitor<'a>(&'a mut String);

    impl Visit for StringVisitor<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                let _ = write!(self.0, " {value:?}");
            } else {
                let _ = write!(self.0, " {}={value:?}", field.name());
            }
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                let _ = write!(self.0, " {value}");
            } else {
                let _ = write!(self.0, " {}={value}", field.name());
            }
        }
    }

    impl<S> Layer<S> for CaptureLayer
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
            let mut line = format!("span: {}", attrs.metadata().name());
            attrs.record(&mut StringVisitor(&mut line));
            self.push(line);
        }

        fn on_record(&self, span: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
            let name = ctx
                .span(span)
                .map_or_else(|| "<unknown>".to_string(), |s| s.name().to_string());
            let mut line = format!("record: {name}");
            values.record(&mut StringVisitor(&mut line));
            self.push(line);
        }

        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut line = String::from("event:");
            event.record(&mut StringVisitor(&mut line));
            self.push(line);
        }
    }

    /// Snapshot the buffer as a single newline-joined string.
    pub fn dump(buf: &Buffer) -> String {
        buf.lock()
            .expect("capture buffer mutex poisoned")
            .join("\n")
    }
}

#[cfg(all(feature = "rustls-tls", feature = "tracing"))]
fn install_capture(buf: capture::Buffer) -> tracing::dispatcher::DefaultGuard {
    use tracing_subscriber::layer::SubscriberExt;

    // Per-test scoped dispatcher. `set_default` returns a guard that
    // resets the per-thread dispatcher when dropped, so each test starts
    // with a clean capture buffer and there is no global mutable state to
    // serialize. The Layer covers all threads via the
    // `tracing_subscriber::registry::Registry` machinery.
    let subscriber =
        tracing_subscriber::registry::Registry::default().with(capture::CaptureLayer::new(buf));
    tracing::subscriber::set_default(subscriber)
}

#[cfg(all(feature = "rustls-tls", feature = "tracing"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_execute_emits_pool_execute_span_with_tier_field() {
    use common::spawn_mock_pool;

    // Arrange — install the capture layer for this test's scope, spawn
    // a multi-connection mock, and build a 2-slot pool against it.
    let buf = capture::new_buffer();
    let _guard = install_capture(buf.clone());

    let (pool, _mock) = spawn_mock_pool(2).await;

    // Act — drive a single SELECT through the §7.3 routing scan. `Box::pin`
    // wraps the future per the `clippy::large_futures` precedent
    // established in `tests/common/mod.rs::spawn_mock_pool`.
    let _ = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("execute ok");

    // Assert — the dump is a newline-joined snapshot of every captured
    // entry (span open, record, event). `contains` is a substring match,
    // case-sensitive. `tracing::instrument` defaults the span name to the
    // bare function name (`execute`); the `tier` field is declared
    // `Empty` on the `instrument(...)` attribute and populated via
    // `Span::current().record("tier", ...)`.
    let logs = capture::dump(&buf);

    assert!(
        logs.contains("span: execute"),
        "expected `Pool::execute` span (recorded as `execute` by `tracing::instrument`) in captured logs:\n{logs}"
    );
    assert!(
        logs.contains("record: execute"),
        "expected `record:` line for `execute` span (tier field update) in captured logs:\n{logs}"
    );
    // The `tier` field is `Empty`-declared at instrument time and filled
    // in via `Span::current().record("tier", ...)` once routing decides
    // which §7.3 tier handled the request. Any of the three valid values
    // is acceptable here — the test verifies the *field* fires, not which
    // tier wins. (A cold pool with `starting_size = 0` lands on
    // `fair_queue` because both `try_idle` and `least_busy_scan` are
    // gated on a non-empty registry; pre-populating with `starting_size`
    // would shift the winner to `try_idle`. The test stays decoupled from
    // that choice.)
    let tier_recorded = logs.contains("tier=try_idle")
        || logs.contains("tier=least_busy_scan")
        || logs.contains("tier=fair_queue");
    assert!(
        tier_recorded,
        "expected `tier` field to be recorded with one of `try_idle` / \
         `least_busy_scan` / `fair_queue` in captured logs:\n{logs}"
    );
}

#[cfg(all(feature = "rustls-tls", feature = "tracing"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserved_drop_emits_trace_event_with_rolled_back() {
    use common::spawn_mock_pool;

    // Arrange — install the capture layer; build a single-connection pool
    // so the reserved acquire path takes the canonical Drop sequence.
    let buf = capture::new_buffer();
    let _guard = install_capture(buf.clone());

    let (pool, _mock) = spawn_mock_pool(1).await;

    // Act — acquire a Reserved with rollback-on-drop armed, then drop it
    // synchronously. The trace event fires from the synchronous Drop
    // impl wired up in Task 8 (`src/pool/reserved.rs`).
    {
        let conn = Box::pin(pool.acquire())
            .await
            .expect("acquire ok")
            .rollback_on_drop();
        drop(conn);
    }

    let logs = capture::dump(&buf);

    // The trace event message is the literal "Reserved dropped" string;
    // the `rolled_back` flag is structured (captured at the moment of Drop).
    assert!(
        logs.contains("event:"),
        "expected at least one captured `event:` line:\n{logs}"
    );
    assert!(
        logs.contains("Reserved dropped"),
        "expected `Reserved dropped` trace event message in captured logs:\n{logs}"
    );
    assert!(
        logs.contains("rolled_back=true"),
        "expected `rolled_back=true` field on the Reserved Drop event in captured logs:\n{logs}"
    );
}
