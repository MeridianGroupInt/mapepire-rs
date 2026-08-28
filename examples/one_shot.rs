//! One-shot SQL via the routed pool.
//!
//! The 80%-of-users path: build a `Pool`, call `pool.execute(sql)`, iterate
//! the rows. The pool's §7.3 routing scan picks an idle connection (or
//! warms a fresh one) on every call.
//!
//! Direct, webpki-trusted cert:
//! ```text
//! MAPEPIRE_HOST=ibmi.example \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//!     cargo run --example one_shot
//! ```
//!
//! Self-signed IBM i over an SSH tunnel — pin the leaf DER, set the TCP hop:
//! ```text
//! MAPEPIRE_HOST=ibmi.example \
//! MAPEPIRE_CONNECT_ADDRESS=127.0.0.1 \
//! MAPEPIRE_PORT=8076 \
//! MAPEPIRE_CA=/path/to/daemon.der \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//! MAPEPIRE_PROPS="access=read only" \
//!     cargo run --example one_shot
//! ```
//!
//! IBM i CN-only certs: default `rustls-tls` + `TlsConfig::Ca` (leaf pin).
//! Do not enable `insecure-tls` for that.

use mapepire::{DaemonServer, Pool, TlsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some((host, user, password)) = read_creds() else {
        eprintln!("Set MAPEPIRE_HOST, MAPEPIRE_USER, and MAPEPIRE_PASSWORD before running.");
        std::process::exit(1);
    };

    let mut b = DaemonServer::builder()
        .host(host)
        .user(user)
        .password(password)
        .tls(tls_from_env());
    if let Ok(addr) = std::env::var("MAPEPIRE_CONNECT_ADDRESS") {
        b = b.connect_address(addr);
    }
    if let Ok(props) = std::env::var("MAPEPIRE_PROPS") {
        b = b.jdbc_props(props);
    }
    if let Ok(port) = std::env::var("MAPEPIRE_PORT") {
        b = b.port(port.parse()?);
    }
    let server = b.build()?;

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

/// `TlsConfig::Verified` unless `MAPEPIRE_CA` is a path to a DER leaf pin.
fn tls_from_env() -> TlsConfig {
    match std::env::var("MAPEPIRE_CA") {
        Ok(path) => match std::fs::read(&path) {
            Ok(der) => TlsConfig::Ca(der),
            Err(e) => {
                eprintln!("failed to read MAPEPIRE_CA={path}: {e}");
                std::process::exit(1);
            }
        },
        Err(_) => TlsConfig::Verified,
    }
}
