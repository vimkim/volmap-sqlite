use super::{SidecarEvidence, page_size_valid, read, word};
use serde::Serialize;
use std::fs::File;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEvidence {
    pub header_valid: bool,
    pub exceeds_hot_size: bool,
    pub hot_status: &'static str,
    pub reserved_lock: &'static str,
    pub super_journal: &'static str,
}

pub(super) fn inspect(file: &File, evidence: &mut SidecarEvidence, main_page_size: u32) {
    let length = evidence.length.parse::<u64>().expect("file length");
    let mut header = [0; 28];
    let count = read(file, &mut header, 0);
    evidence.evaluated(count as u64);
    evidence.coverage.scope = "journal_header";
    evidence.coverage.reason = "validation_stop";
    evidence.state = "malformed";
    evidence.fields(
        &header[..count],
        &[
            ("page_count", 8),
            ("checksum_nonce", 12),
            ("initial_database_pages", 16),
            ("sector_size", 20),
            ("page_size", 24),
        ],
        0,
        true,
    );
    let mut journal = JournalEvidence {
        header_valid: false,
        exceeds_hot_size: length > 512,
        hot_status: if length <= 512 { "not_hot" } else { "unknown" },
        reserved_lock: "unknown",
        super_journal: "not_inspected",
    };
    if length == 0 || (count >= 8 && header[..8] == [0; 8]) {
        evidence.state = "inactive";
        evidence.coverage.reason = "supported_scope_complete";
        journal.hot_status = "not_hot";
    } else if count < 28 {
        evidence.diagnostic("journal_header_truncated", 0, count as u64);
    } else if header[..8] != [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7] {
        evidence.diagnostic("journal_magic", 0, 8);
    } else {
        let sector = word(&header, 20, true);
        let page_size = word(&header, 24, true);
        if !(32..=65536).contains(&sector) || !sector.is_power_of_two() {
            evidence.diagnostic("journal_sector_size", 20, 4);
        } else if !page_size_valid(page_size) {
            evidence.diagnostic("journal_page_size", 24, 4);
        } else if u64::from(sector) > length {
            evidence.diagnostic("journal_header_sector_truncated", 0, length);
        } else {
            journal.header_valid = true;
            evidence.state = "supported_header";
            evidence.coverage.reason = "supported_scope_complete";
            if page_size != main_page_size {
                evidence.state = "conflicting";
                evidence.diagnostic("journal_main_page_size_mismatch", 24, 4);
            }
            let records = word(&header, 8, true);
            if records != u32::MAX
                && u64::from(sector) + u64::from(records) * (u64::from(page_size) + 8) > length
            {
                evidence.state = "malformed";
                evidence.diagnostic("journal_declared_records_truncated", 8, 4);
            }
        }
    }
    // Lock bytes in a copied file do not establish OS/VFS lock ownership. Never follow
    // a database-controlled super-journal path or assert recovery eligibility from it.
    evidence.journal = Some(journal);
}
