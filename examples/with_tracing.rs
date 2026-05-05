//! `tracing` instrumentation in action.
//!
//! Enable the `tracing` Cargo feature, register any `tracing-subscriber`
//! sink, and every public dispatch entry point (`Pool::execute`,
//! `Pool::execute_with`, `Pool::acquire`, `Job::execute*`,
//! `Reserved::*`) emits spans with `sql`, `param_count`, and (for
//! `Pool::execute*`) the routing-tier label `tier ∈ {try_idle,
//! least_busy_scan, fair_queue}`.
//!
//! Run:
//! ```text
//! MAPEPIRE_HOST=ibmi.example.com \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//!     cargo run --example with_tracing --features tracing
//! ```

use mapepire::{DaemonServer, Pool, TlsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Stdout fmt subscriber. RUST_LOG=mapepire=debug for span entry/exit
    // events; default INFO is enough to see the executions themselves.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mapepire=info")),
        )
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .init();

    let Some((host, user, password)) = read_creds() else {
        eprintln!("Set MAPEPIRE_HOST, MAPEPIRE_USER, and MAPEPIRE_PASSWORD before running.");
        std::process::exit(1);
    };

    let server = DaemonServer::builder()
        .host(host)
        .user(user)
        .password(password)
        .tls(TlsConfig::Verified)
        .build()?;

    let pool = Pool::builder(server).max_size(2).build().await?;

    // First execute warms the pool — fair-queue tier (cold start).
    drop(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1").await?);

    // Second execute hits the registry-backed try-idle fast path.
    drop(pool.execute("SELECT 2 FROM SYSIBM.SYSDUMMY1").await?);

    Ok(())
}

fn read_creds() -> Option<(String, String, String)> {
    Some((
        std::env::var("MAPEPIRE_HOST").ok()?,
        std::env::var("MAPEPIRE_USER").ok()?,
        std::env::var("MAPEPIRE_PASSWORD").ok()?,
    ))
}
