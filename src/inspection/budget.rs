//! Operator ceilings shared by every inspection adapter.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationalBudget {
    /// Aggregate structural cells accepted by the page inventory.
    /// A page is a validation boundary: it is accepted in full or left unevaluated.
    pub max_processed_cells: u64,
    /// Maximum evaluated work units in any structural phase.
    pub max_phase_units: u64,
    /// Process resident-memory ceiling, checked before allocation boundaries.
    pub max_resident_bytes: u64,
    pub max_freelist_trunks: u32,
}

impl Default for OperationalBudget {
    fn default() -> Self {
        Self {
            max_processed_cells: 1_000_000,
            max_phase_units: 1_000_000,
            max_resident_bytes: 256 * 1024 * 1024,
            max_freelist_trunks: 32768,
        }
    }
}

/// Includes process/runtime residency. Failure to measure fails closed.
pub(super) fn memory_available(ceiling: u64, reservation: u64) -> bool {
    resident_bytes()
        .and_then(|resident| resident.checked_add(reservation))
        .is_some_and(|required| required <= ceiling)
}

pub(super) fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

/// Count serialized bytes without allocating an intermediate JSON document.
pub(super) fn serialized_reservation(value: &impl Serialize) -> Option<u64> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len() as u64);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.0.saturating_mul(4))
}
