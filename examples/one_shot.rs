//! One-shot SQL via the routed pool.
//!
//! The 80%-of-users path: build a `Pool`, call `pool.execute(sql)`, iterate
//! the rows. The pool's §7.3 routing scan picks an idle connection (or
//! warms a fresh one) on every call.
//!
//! Run:
//! ```text
//! MAPEPIRE_HOST=ibmi.example.com \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//!     cargo run --example one_shot
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

    let pool = Pool::builder(server).max_size(4).build().await?;

    // SYSIBM.SYSDUMMY1 is the standard Db2 single-row sanity table — one
    // CHAR(1) column called IBMREQD. Every IBM i system has it.
    let rows = pool.execute("SELECT IBMREQD FROM SYSIBM.SYSDUMMY1").await?;

    let dynamic = rows.into_dynamic().await?;
    for row in dynamic {
        let value: String = row.get("IBMREQD")?;
        println!("IBMREQD = {value}");
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
