//! Typed row streaming via `Rows::stream_typed::<T>()`.
//!
//! Each page returned by `sqlmore` is decoded into `T: FromRow`. The
//! blanket impl on `T: serde::Deserialize` covers the common case —
//! derive `Deserialize` and rename fields to match the column names.
//!
//! Run:
//! ```text
//! MAPEPIRE_HOST=ibmi.example.com \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//!     cargo run --example streaming
//! ```

use futures::TryStreamExt;
use mapepire::{DaemonServer, Pool, TlsConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Library {
    #[serde(rename = "ODOBNM")]
    name: String,
    #[serde(rename = "ODOBTP")]
    object_type: String,
}

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

    // QSYS2.OBJECT_STATISTICS is a table function; calling against
    // *ALLUSR / *LIB lists user libraries with type *LIB.
    let rows = pool
        .execute(
            "SELECT ODOBNM, ODOBTP \
             FROM TABLE(QSYS2.OBJECT_STATISTICS('*ALLUSR ', '*LIB')) X \
             FETCH FIRST 5 ROWS ONLY",
        )
        .await?;

    // try_collect drains the stream, propagating the first error if
    // any page fails to decode.
    let libraries: Vec<Library> = rows.stream_typed::<Library>().try_collect().await?;

    for lib in &libraries {
        println!("{} ({})", lib.name, lib.object_type);
    }
    println!("({} rows)", libraries.len());

    Ok(())
}

fn read_creds() -> Option<(String, String, String)> {
    Some((
        std::env::var("MAPEPIRE_HOST").ok()?,
        std::env::var("MAPEPIRE_USER").ok()?,
        std::env::var("MAPEPIRE_PASSWORD").ok()?,
    ))
}
