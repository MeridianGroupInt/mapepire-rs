//! Run an IBM i CL command via `Job::cl`.
//!
//! `Job::cl(command)` sends the command through the daemon and returns
//! the first [`mapepire::ClMessage`] from the response (typically the
//! CPF completion or escape message). Uses `Job::connect` directly —
//! no pool needed for a one-shot command.
//!
//! Run:
//! ```text
//! MAPEPIRE_HOST=ibmi.example.com \
//! MAPEPIRE_USER=YOURUSER \
//! MAPEPIRE_PASSWORD=secret \
//!     cargo run --example cl_command
//! ```

use mapepire::{DaemonServer, Job, TlsConfig};

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

    let job = Job::connect(&server).await?;

    // DSPLIB QGPL is a benign read-only command available on every IBM i
    // — it emits a CPF2102 completion message.
    let msg = job.cl("DSPLIB QGPL").await?;

    println!("id   = {:?}", msg.id);
    println!("kind = {:?}", msg.kind);
    println!("text = {:?}", msg.text);

    Ok(())
}

fn read_creds() -> Option<(String, String, String)> {
    Some((
        std::env::var("MAPEPIRE_HOST").ok()?,
        std::env::var("MAPEPIRE_USER").ok()?,
        std::env::var("MAPEPIRE_PASSWORD").ok()?,
    ))
}
