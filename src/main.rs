#![forbid(unsafe_code)]

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::net::TcpListener;
use volmap_sqlite::inspection::{InspectionSession, ScanControl};
use volmap_sqlite::web::atlas_router;

#[derive(Debug, Parser)]
#[command(name = "volmap-sqlite", about = "Inspect a frozen SQLite main file")]
struct Arguments {
    /// Frozen `SQLite` main file to inspect.
    database: PathBuf,

    /// Address for the embedded browser viewer.
    #[arg(long, default_value = "127.0.0.1:3000")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let session = Arc::new(InspectionSession::begin(&arguments.database)?);
    let display_name = session.status().source.display_name;

    if !arguments.listen.ip().is_loopback() {
        eprintln!(
            "warning: the viewer is listening beyond loopback and provides no authentication or TLS"
        );
    }

    let listener = TcpListener::bind(arguments.listen).await?;
    let address = listener.local_addr()?;
    eprintln!("Inspecting {display_name}");
    eprintln!("Page atlas: http://{address}/");
    let scan_session = Arc::clone(&session);
    let worker = tokio::task::spawn_blocking(move || scan_session.scan(|_| ScanControl::Continue));
    let shutdown_session = Arc::clone(&session);
    let server = axum::serve(listener, atlas_router(session))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = shutdown_session.stop(ScanControl::Cancel);
        })
        .await;
    let _ = worker.await?;
    server?;
    Ok(())
}
