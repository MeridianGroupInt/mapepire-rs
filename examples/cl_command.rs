//! Run an IBM i CL command via `Job::cl`.
//!
//! `Job::cl(command)` sends the command through the daemon and returns a
//! [`mapepire::ClOutcome`] with the full job log. Failed commands (for
//! example CPF0006) are `Ok` with `success: false` — they do not become
//! `Err`. Uses `Job::connect` directly — no pool needed for a one-shot
//! command.
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

    // DSPLIB QGPL is a benign read-only command available on every IBM i.
    let outcome = job.cl("DSPLIB QGPL").await?;

    println!("success  = {}", outcome.success);
    println!("error    = {:?}", outcome.error);
    println!("sqlcode  = {:?}", outcome.sqlcode);
    println!("sqlstate = {:?}", outcome.sqlstate);
    for entry in &outcome.entries {
        println!(
            "{} [{}] {}",
            entry.message_id.as_deref().unwrap_or("?"),
            entry.severity.as_deref().unwrap_or("?"),
            entry.message_text.as_deref().unwrap_or("")
        );
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
