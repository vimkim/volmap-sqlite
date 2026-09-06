#![forbid(unsafe_code)]

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::net::TcpListener;
use volmap_sqlite::inspection::InspectionSession;
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
    let session = Arc::new(InspectionSession::open(&arguments.database)?);
    let display_name = &session.graph().snapshot.source.display_name;

    if !arguments.listen.ip().is_loopback() {
        eprintln!(
            "warning: the viewer is listening beyond loopback and provides no authentication or TLS"
        );
    }

    let listener = TcpListener::bind(arguments.listen).await?;
    let address = listener.local_addr()?;
    eprintln!("Inspecting {display_name}");
    eprintln!("Page atlas: http://{address}/");
    axum::serve(listener, atlas_router(session)).await?;
    Ok(())
}
