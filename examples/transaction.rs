//! Transactions with the v0.4 in-tx-only `rollback_on_drop` contract.
//!
//! `pool.acquire()` returns a [`mapepire::Reserved`]; chaining
//! `.rollback_on_drop()` arms a best-effort `ROLLBACK` on Drop. As of
//! v0.4 the Drop fires *only* when a `BEGIN` was observed without a
//! matching `COMMIT`/`ROLLBACK` — so the explicit `commit()` below
//! suppresses the rollback path entirely. If we panic or early-return
//! between `begin()` and `commit()`, Drop fires the rollback.
//!
//! Run:
//! ```text
//! MAPEPIRE_HOST=ibmi.example.com \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//!     cargo run --example transaction
//! ```

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

    // Acquire pins one connection. `.rollback_on_drop()` is the safety
    // net for early returns between begin() and commit().
    let conn = pool.acquire().await?.rollback_on_drop();

    // v0.4 typed transaction helpers — funnel through Reserved::execute
    // so the internal TxState machine stays accurate (which gates the
    // Drop rollback path).
    conn.begin().await?;

    // The sample work below targets a hypothetical SCRATCH.DEMO table.
    // Replace with your own DML; everything between begin() and commit()
    // rides the same socket.
    //
    // If any step here returns Err, the `?` propagates and the Reserved
    // drops with tx_state == Started — Drop fires ROLLBACK best-effort
    // and the (un-COMMITed) work is discarded by the daemon.
    conn.execute_with(
        "INSERT INTO SCRATCH.DEMO (ID, NAME) VALUES (?, ?)",
        &[serde_json::json!(1), serde_json::json!("first")],
    )
    .await?;
    conn.execute_with(
        "UPDATE SCRATCH.DEMO SET NAME = ? WHERE ID = ?",
        &[serde_json::json!("renamed"), serde_json::json!(1)],
    )
    .await?;

    conn.commit().await?;
    // After commit(), tx_state == Closed. The Drop on `conn` is a no-op
    // for ROLLBACK; the connection returns to the pool cleanly.

    println!("Committed.");
    Ok(())
}

fn read_creds() -> Option<(String, String, String)> {
    Some((
        std::env::var("MAPEPIRE_HOST").ok()?,
        std::env::var("MAPEPIRE_USER").ok()?,
        std::env::var("MAPEPIRE_PASSWORD").ok()?,
    ))
}
