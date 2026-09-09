#![forbid(unsafe_code)]

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::net::TcpListener;
use volmap_sqlite::inspection::{InspectionSession, ScanControl, SidecarBudget, TraversalBudget};
use volmap_sqlite::web::atlas_router;

#[derive(Debug, Parser)]
#[command(name = "volmap-sqlite", about = "Inspect a frozen SQLite main file")]
struct Arguments {
    /// Frozen `SQLite` main file to inspect.
    database: PathBuf,

    /// Address for the embedded browser viewer.
    #[arg(long, default_value = "127.0.0.1:3000")]
    listen: SocketAddr,

    /// Maximum number of pages retained in each B-tree traversal prefix (minimum 1).
    #[arg(long, default_value_t = u32::MAX, value_parser = clap::value_parser!(u32).range(1..))]
    max_btree_pages: u32,

    /// Maximum number of pages retained in each overflow-chain prefix.
    #[arg(long, default_value_t = u32::MAX)]
    max_overflow_pages: u32,

    /// Maximum aggregate page identities allocated across all traversal prefixes (minimum 1).
    #[arg(long, default_value_t = 1_000_000, value_parser = clap::value_parser!(u64).range(1..))]
    max_total_traversal_pages: u64,

    /// Maximum WAL frames retained as sidecar evidence (0 inspects only the header).
    #[arg(long, default_value_t = 100_000)]
    max_wal_frames: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let session = Arc::new(InspectionSession::begin_with_budgets(
        &arguments.database,
        TraversalBudget::with_total_pages(
            arguments.max_btree_pages,
            arguments.max_overflow_pages,
            arguments.max_total_traversal_pages,
        ),
        SidecarBudget {
            max_wal_frames: arguments.max_wal_frames,
        },
    )?);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_limits_are_explicit_cli_inputs() {
        let arguments = Arguments::try_parse_from([
            "volmap-sqlite",
            "fixture.sqlite",
            "--max-btree-pages",
            "12",
            "--max-overflow-pages",
            "34",
            "--max-total-traversal-pages",
            "56",
        ])
        .unwrap();

        assert_eq!(arguments.max_btree_pages, 12);
        assert_eq!(arguments.max_overflow_pages, 34);
        assert_eq!(arguments.max_total_traversal_pages, 56);
    }

    #[test]
    fn traversal_limits_reject_values_without_a_representable_boundary() {
        for flag in ["--max-btree-pages", "--max-total-traversal-pages"] {
            assert!(
                Arguments::try_parse_from(["volmap-sqlite", "fixture.sqlite", flag, "0"]).is_err()
            );
        }
    }
}
