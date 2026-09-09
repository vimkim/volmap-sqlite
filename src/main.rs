#![forbid(unsafe_code)]

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::net::TcpListener;
use volmap_sqlite::inspection::{
    InspectionSession, ScanControl, SchemaBudget, SidecarBudget, TraversalBudget,
};
use volmap_sqlite::web::atlas_router;

#[derive(Debug, Parser)]
#[command(name = "volmap-sqlite", about = "Inspect a frozen SQLite main file")]
struct Arguments {
    /// Frozen `SQLite` main file to inspect.
    database: PathBuf,

    /// Launch the keyboard-oriented terminal inspection flow.
    #[arg(long)]
    terminal: bool,

    /// Terminal deep-inspection payload-byte request budget.
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    max_deep_bytes: u64,

    /// Terminal deep-inspection overflow-page request budget.
    #[arg(long, default_value_t = 32768)]
    max_deep_overflow_pages: u32,

    /// Terminal deep-inspection decoded-value request budget.
    #[arg(long, default_value_t = 4096)]
    max_deep_values: u32,

    /// Enable bounded, descriptive `SQLite` schema metadata.
    #[arg(long)]
    semantic_metadata: bool,

    /// Maximum bytes copied for optional metadata (security ceiling: 64 MiB).
    #[arg(long, default_value_t = 64 * 1024 * 1024)]
    max_semantic_bytes: u64,

    /// Maximum schema records considered for optional metadata (ceiling: 1024).
    #[arg(long, default_value_t = 1024)]
    max_semantic_records: usize,

    /// Optional metadata wall-clock budget in milliseconds (ceiling: 2000).
    #[arg(long, default_value_t = 2000)]
    max_semantic_ms: u64,

    /// Maximum optional metadata protocol bytes (ceiling: 256 KiB).
    #[arg(long, default_value_t = 256 * 1024)]
    max_semantic_output_bytes: u64,

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

    /// Maximum aggregate schema-record payload bytes decoded (0 disables schema decoding).
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    max_schema_bytes: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("--private-semantic-helper") {
        std::process::exit(volmap_sqlite::semantic::run_private_helper());
    }
    let arguments = Arguments::parse();
    let session = InspectionSession::begin_with_schema_budget(
        &arguments.database,
        TraversalBudget::with_total_pages(
            arguments.max_btree_pages,
            arguments.max_overflow_pages,
            arguments.max_total_traversal_pages,
        ),
        SidecarBudget {
            max_wal_frames: arguments.max_wal_frames,
        },
        SchemaBudget {
            max_decoded_bytes: arguments.max_schema_bytes,
        },
    )?;
    let session = Arc::new(if arguments.semantic_metadata {
        session.with_semantic_metadata_budget(volmap_sqlite::semantic::SemanticBudget {
            max_copy_bytes: arguments.max_semantic_bytes,
            max_schema_records: arguments.max_semantic_records,
            timeout_ms: arguments.max_semantic_ms,
            max_output_bytes: arguments.max_semantic_output_bytes,
        })
    } else {
        session
    });
    if arguments.terminal {
        volmap_sqlite::terminal::run(
            session,
            volmap_sqlite::inspection::DeepBudget {
                max_payload_bytes: arguments.max_deep_bytes,
                max_overflow_pages: arguments.max_deep_overflow_pages,
                max_values: arguments.max_deep_values,
            },
        )?;
        return Ok(());
    }
    serve(session, arguments.listen)
}

#[tokio::main]
async fn serve(session: Arc<InspectionSession>, listen: SocketAddr) -> Result<(), Box<dyn Error>> {
    let display_name = session.status().source.display_name;

    if !listen.ip().is_loopback() {
        eprintln!(
            "warning: the viewer is listening beyond loopback and provides no authentication or TLS"
        );
    }

    let listener = TcpListener::bind(listen).await?;
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
