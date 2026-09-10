//! Reproducible process-level observation of default inspection budgets.
use std::error::Error;
use std::time::Instant;

use rusqlite::Connection;
use volmap_sqlite::inspection::{InspectionSession, ScanControl};

fn main() -> Result<(), Box<dyn Error>> {
    let rows: u32 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "20000".into())
        .parse()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("budget-probe.sqlite");
    let mut database = Connection::open(&path)?;
    database.execute_batch("PRAGMA page_size=4096; CREATE TABLE entries(id INTEGER PRIMARY KEY, label TEXT); CREATE INDEX labels ON entries(label);")?;
    let transaction = database.transaction()?;
    {
        let mut insert = transaction.prepare("INSERT INTO entries VALUES(?1, ?2)")?;
        for number in 0..rows {
            insert.execute(rusqlite::params![number, format!("entry-{number:08}")])?;
        }
    }
    transaction.commit()?;
    drop(database);
    let start = Instant::now();
    let session = InspectionSession::begin(&path)?;
    session.scan(|_| ScanControl::Continue)?;
    let graph = session.revision_summary(1)?;
    let mut first = 1;
    let mut cells = 0;
    loop {
        let batch = session.page_batch(1, first, 128)?;
        cells += batch
            .pages
            .iter()
            .map(|page| page.detail.cells.len())
            .sum::<usize>();
        match batch.next_page {
            Some(next) => first = next,
            None => break,
        }
    }
    let status = std::fs::read_to_string("/proc/self/status")?;
    let peak = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .unwrap_or("unknown");
    println!(
        "rows={rows} pages={} cells={} elapsed_ms={} {peak}",
        graph.page_count,
        cells,
        start.elapsed().as_millis()
    );
    println!(
        "inventory={:?} topology={:?} schema={:?} budgets={:?}",
        graph.coverage.reason,
        graph.topology_coverage.reason,
        graph.schema_state,
        session.status().operational_budget
    );
    println!("storage={:?}", session.status().storage);
    Ok(())
}
