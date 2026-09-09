//! Read-only disclosure. Sidecar page references never enter main-file topology.
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

use serde::Serialize;

mod shm;
pub use shm::ShmEvidence;
mod journal;
pub use journal::JournalEvidence;
mod wal;
pub use wal::{WalEvidence, WalFrame};

/// Resource ceiling for retained WAL frame evidence, independent of main-file traversal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SidecarBudget {
    pub max_wal_frames: u64,
}
impl Default for SidecarBudget {
    fn default() -> Self {
        Self {
            max_wal_frames: 100_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarEvidence {
    pub kind: &'static str,
    pub display_name: String,
    pub length: String,
    pub state: &'static str,
    pub consequence: &'static str,
    pub fields: Vec<SidecarField>,
    pub wal: Option<WalEvidence>,
    pub journal: Option<JournalEvidence>,
    pub shm: Option<ShmEvidence>,
    pub diagnostics: Vec<SidecarDiagnostic>,
    pub coverage: SidecarCoverage,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarField {
    pub name: &'static str,
    pub value: u32,
    pub offset: String,
    pub length: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarDiagnostic {
    pub code: &'static str,
    pub offset: String,
    pub length: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarCoverage {
    pub scope: &'static str,
    pub reason: &'static str,
    pub evaluated_bytes: String,
    pub remaining_bytes: String,
}

impl SidecarEvidence {
    pub(super) fn discovered(kind: &'static str, path: &Path, length: u64, present: bool) -> Self {
        Self {
            kind,
            display_name: super::sanitized_display_name(path),
            length: length.to_string(),
            state: if present { "uninspected" } else { "absent" },
            consequence: if present {
                match kind {
                    "wal" => "wal_not_applied",
                    "journal" => "rollback_not_applied",
                    _ => "shm_non_authoritative",
                }
            } else {
                "none"
            },
            fields: Vec::new(),
            wal: None,
            journal: None,
            shm: None,
            diagnostics: Vec::new(),
            coverage: SidecarCoverage {
                scope: "presence",
                reason: if present { "not_inspected" } else { "complete" },
                evaluated_bytes: "0".into(),
                remaining_bytes: length.to_string(),
            },
        }
    }

    fn diagnostic(&mut self, code: &'static str, offset: u64, length: u64) {
        self.diagnostics.push(SidecarDiagnostic {
            code,
            offset: offset.to_string(),
            length: length.to_string(),
        });
    }
}

pub(super) fn inspect(
    path: &Path,
    mut evidence: SidecarEvidence,
    page_size: u32,
    control: &super::topology::WorkControl,
    checkpoint: &mut dyn FnMut(),
    budget: SidecarBudget,
) -> SidecarEvidence {
    if evidence.state != "uninspected" {
        return evidence;
    }
    let Ok(file) = File::open(path) else {
        evidence.unreadable();
        return evidence;
    };
    if evidence.kind == "wal" {
        wal::inspect(&file, &mut evidence, page_size, control, checkpoint, budget);
    } else if evidence.kind == "journal" {
        journal::inspect(&file, &mut evidence, page_size);
    } else {
        shm::inspect(&file, &mut evidence, page_size);
    }
    evidence
}

fn read(file: &File, buffer: &mut [u8], offset: u64) -> usize {
    let mut count = 0;
    while count < buffer.len() {
        match file.read_at(&mut buffer[count..], offset + count as u64) {
            Ok(0) => break,
            Ok(n) => count += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    count
}

fn word(bytes: &[u8], offset: usize, big: bool) -> u32 {
    let value = bytes[offset..offset + 4].try_into().expect("bounded field");
    if big {
        u32::from_be_bytes(value)
    } else {
        u32::from_le_bytes(value)
    }
}

fn checksum(bytes: &[u8], big: bool, mut sum: [u32; 2]) -> [u32; 2] {
    for pair in bytes.chunks_exact(8) {
        sum[0] = sum[0].wrapping_add(word(pair, 0, big)).wrapping_add(sum[1]);
        sum[1] = sum[1].wrapping_add(word(pair, 4, big)).wrapping_add(sum[0]);
    }
    sum
}

fn page_size_valid(size: u32) -> bool {
    (512..=65536).contains(&size) && size.is_power_of_two()
}

impl SidecarEvidence {
    pub(super) fn unreadable(&mut self) {
        self.state = "unreadable";
        self.coverage.reason = "unreadable";
        self.diagnostic("sidecar_unreadable", 0, 0);
    }

    fn evaluated(&mut self, bytes: u64) {
        self.coverage.evaluated_bytes = bytes.to_string();
        self.coverage.remaining_bytes = self
            .length
            .parse::<u64>()
            .expect("file length")
            .saturating_sub(bytes)
            .to_string();
    }

    fn fields(&mut self, bytes: &[u8], names: &[(&'static str, usize)], base: u64, big: bool) {
        for &(name, offset) in names {
            if offset + 4 <= bytes.len() {
                self.fields.push(SidecarField {
                    name,
                    value: word(bytes, offset, big),
                    offset: (base + offset as u64).to_string(),
                    length: 4,
                });
            }
        }
    }
}
