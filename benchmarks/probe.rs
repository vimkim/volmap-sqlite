//! Runs only inspection; fixture generation belongs in a separate process.
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use volmap_sqlite::inspection::{
    InspectionSession, OperationalBudget, ScanControl, StorageBudget, TraversalBudget,
};

pub fn run(
    path: &Path,
    resident_bytes: u64,
    cache_bytes: u64,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let started = Instant::now();
    let session = Arc::new(
        InspectionSession::begin_with_traversal_budget(
            path,
            TraversalBudget::with_total_pages(256, 1_000_000, 16_000_000),
        )?
        .with_operational_budget(OperationalBudget {
            max_resident_bytes: resident_bytes,
            max_processed_cells: u64::MAX,
            max_phase_units: u64::MAX,
            ..Default::default()
        })
        .with_storage_budget(StorageBudget {
            cache_bytes,
            ..Default::default()
        }),
    );
    session.scan(|_| ScanControl::Continue)?;
    let scan_ms = started.elapsed().as_millis();
    let summary = session.revision_summary(1)?;
    let first = session.page_batch(1, 1, 1);
    let last = session.page_batch(1, summary.page_count.max(1), 1);
    let navigation = if first.is_ok() && last.is_ok() && summary.page_count > 0 {
        let navigation_started = Instant::now();
        let mut terminal = volmap_sqlite::terminal::TerminalFlow::new(Arc::clone(&session));
        terminal.key(volmap_sqlite::terminal::Key::Char('g'));
        for character in summary.page_count.to_string().chars() {
            terminal.key(volmap_sqlite::terminal::Key::Char(character));
        }
        terminal.key(volmap_sqlite::terminal::Key::Enter);
        let frame = terminal.screen(160, 80).join("\n");
        Some(serde_json::json!({
            "elapsedMs": navigation_started.elapsed().as_millis(),
            "page": summary.page_count,
            "available": frame.contains(&format!("Page {}", summary.page_count))
                && !frame.contains("Evidence window unavailable")
                && !frame.contains("Invalid selector"),
        }))
    } else {
        None
    };
    let process = std::fs::read_to_string("/proc/self/status")?;
    let peak_kib: u64 = process
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|line| line.split_whitespace().next())
        .ok_or("missing peak RSS")?
        .parse()?;
    let status = session.status();
    Ok(serde_json::json!({
        "elapsedMs": started.elapsed().as_millis(), "scanMs": scan_ms,
        "terminalNavigation": navigation, "peakResidentBytes": peak_kib * 1024,
        "inputBytes": summary.snapshot.geometry.file_length,
        "pageSize": summary.snapshot.geometry.page_size, "evaluatedPages": summary.page_count,
        "summary": summary, "storage": status.storage,
        "operationalBudget": status.operational_budget, "storageBudget": status.storage_budget,
        "traversalBudget": status.traversal_budget, "workCoverage": status.work_coverage,
        "firstPage": first.as_ref().ok().and_then(|batch| batch.pages.first()).map(|page| page.number),
        "lastPage": last.as_ref().ok().and_then(|batch| batch.pages.first()).map(|page| page.number),
        "firstPageError": first.err().map(|error| error.to_string()),
        "lastPageError": last.err().map(|error| error.to_string()),
    }))
}
