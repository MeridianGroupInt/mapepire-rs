//! Server-side prepared statements via `Job::prepare`.
//!
//! Prepare once, execute many — the dispatcher reuses the same `cont_id`
//! on every call so the daemon doesn't re-parse the SQL. `pool.acquire()`
//! returns a [`mapepire::Reserved`] that derefs to `&Job`, giving us
//! direct access to `Job::prepare` and `Job::ids` for the [`mapepire::Query`]
//! handle.
//!
//! Run:
//! ```text
//! MAPEPIRE_HOST=ibmi.example \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//!     cargo run --example prepared
//! ```
//!
//! Optional tunnel / pin / JDBC vars are documented on
//! [`examples/one_shot.rs`](one_shot.rs): `MAPEPIRE_CONNECT_ADDRESS`,
//! `MAPEPIRE_PORT`, `MAPEPIRE_PROPS`, `MAPEPIRE_CA`.

use mapepire::{DaemonServer, Pool, TlsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Pin one connection so all prepared-statement calls land on the
    // same socket (the prepared statement's `cont_id` is per-connection).
    let conn = pool.acquire().await?;

    // Prepare once.
    let query = conn.prepare("VALUES (CAST(? AS VARCHAR(64)))").await?;

    // Execute many — same `cont_id` on every call.
    for name in ["alpha", "beta", "gamma"] {
        let rows = query
            .execute_with(conn.ids(), &[serde_json::json!(name)])
            .await?;
        let dynamic = rows.into_dynamic().await?;
        for row in dynamic {
            // VALUES yields a single column whose name is "00001" by
            // default on Db2 for IBM i.
            let s: String = row.get("00001")?;
            println!("{name} → {s}");
        }
    }

    Ok(())
}

fn read_creds() -> Option<(String, String, String)> {
    Some((
        std::env::var("MAPEPIRE_HOST").ok()?,
        std::env::var("MAPEPIRE_USER").ok()?,
        std::env::var("MAPEPIRE_PASSWORD").ok()?,
    ))
}
