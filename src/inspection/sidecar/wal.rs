use super::{SidecarEvidence, checksum, page_size_valid, read, word};
use crate::inspection::topology::WorkControl;
use serde::Serialize;
use std::fs::File;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalEvidence {
    pub frame_limit: String,
    pub checksum_byte_order: &'static str,
    pub validated_frames: String,
    pub last_commit_frame: Option<String>,
    pub frames: Vec<WalFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalFrame {
    pub number: String,
    pub offset: String,
    pub readable_bytes: u32,
    pub page_number: Option<u32>,
    pub database_size: Option<u32>,
    pub salts: Option<[u32; 2]>,
    pub stored_checksum: Option<[u32; 2]>,
    pub checksum_valid: Option<bool>,
    pub state: &'static str,
    pub diagnostic: Option<&'static str>,
}

struct Header {
    size: u32,
    big: bool,
    sum: [u32; 2],
    salts: [u32; 2],
}

fn read_header(file: &File, evidence: &mut SidecarEvidence, frame_limit: u64) -> Option<Header> {
    evidence.coverage.scope = "wal_frames";
    evidence.coverage.reason = "validation_stop";
    evidence.state = "malformed";
    let mut header = [0; 32];
    let count = read(file, &mut header, 0);
    evidence.evaluated(count as u64);
    evidence.fields(
        &header[..count],
        &[
            ("magic", 0),
            ("version", 4),
            ("page_size", 8),
            ("checkpoint_sequence", 12),
            ("salt_1", 16),
            ("salt_2", 20),
            ("checksum_1", 24),
            ("checksum_2", 28),
        ],
        0,
        true,
    );
    if count < 32 {
        evidence.diagnostic("wal_header_truncated", 0, count as u64);
        return None;
    }
    let magic = word(&header, 0, true);
    if ![0x377f_0682, 0x377f_0683].contains(&magic) {
        evidence.diagnostic("wal_magic", 0, 4);
        return None;
    }
    if word(&header, 4, true) != 3_007_000 {
        evidence.state = "unsupported";
        evidence.diagnostic("wal_version", 4, 4);
        return None;
    }
    let size = word(&header, 8, true);
    if !page_size_valid(size) {
        evidence.diagnostic("wal_page_size", 8, 4);
        return None;
    }
    let big = magic == 0x377f_0683;
    let sum = checksum(&header[..24], big, [0, 0]);
    let wal = WalEvidence {
        frame_limit: frame_limit.to_string(),
        checksum_byte_order: if big { "big" } else { "little" },
        validated_frames: "0".into(),
        last_commit_frame: None,
        frames: Vec::new(),
    };
    if sum != [word(&header, 24, true), word(&header, 28, true)] {
        evidence.diagnostic("wal_header_checksum", 24, 8);
        evidence.wal = Some(wal);
        return None;
    }
    evidence.wal = Some(wal);
    Some(Header {
        size,
        big,
        sum,
        salts: [word(&header, 16, true), word(&header, 20, true)],
    })
}

fn read_frame(
    file: &File,
    buffer: &mut [u8],
    offset: u64,
    number: u64,
    header: &mut Header,
) -> WalFrame {
    let count = read(file, buffer, offset);
    let mut frame = WalFrame {
        number: number.to_string(),
        offset: offset.to_string(),
        readable_bytes: u32::try_from(count).expect("bounded frame"),
        page_number: (count >= 4).then(|| word(buffer, 0, true)),
        database_size: (count >= 8).then(|| word(buffer, 4, true)),
        salts: (count >= 16).then(|| [word(buffer, 8, true), word(buffer, 12, true)]),
        stored_checksum: (count >= 24).then(|| [word(buffer, 16, true), word(buffer, 20, true)]),
        checksum_valid: None,
        state: "malformed",
        diagnostic: None,
    };
    frame.diagnostic = if count < buffer.len() {
        Some("wal_frame_truncated")
    } else if frame.salts != Some(header.salts) {
        frame.state = "unresolved";
        Some("wal_salt_mismatch_or_stale_tail")
    } else {
        let candidate = checksum(
            &buffer[24..],
            header.big,
            checksum(&buffer[..8], header.big, header.sum),
        );
        frame.checksum_valid = Some(frame.stored_checksum == Some(candidate));
        if frame.checksum_valid != Some(true) {
            Some("wal_frame_checksum")
        } else if frame.page_number == Some(0)
            || frame.page_number == Some(u32::MAX)
            || frame.database_size == Some(u32::MAX)
        {
            Some("wal_frame_page_number")
        } else {
            header.sum = candidate;
            frame.state = "validated";
            None
        }
    };
    frame
}

pub(super) fn inspect(
    file: &File,
    evidence: &mut SidecarEvidence,
    main_page_size: u32,
    control: &WorkControl,
    checkpoint: &mut dyn FnMut(),
    budget: super::SidecarBudget,
) {
    let Some(mut header) = read_header(file, evidence, budget.max_wal_frames) else {
        return;
    };
    let mut wal = evidence.wal.take().expect("validated WAL header");
    let size_matches = header.size == main_page_size;
    if !size_matches {
        evidence.diagnostic("wal_main_page_size_mismatch", 8, 4);
    }
    let length = evidence.length.parse::<u64>().expect("file length");
    let mut offset = 32;
    let mut number = 1_u64;
    let mut buffer = vec![0; header.size as usize + 24];
    while offset < length {
        checkpoint();
        if let Some(reason) = control.traversal_reason() {
            evidence.state = "partial";
            evidence.coverage.reason =
                if reason == crate::inspection::TraversalStopReason::Cancelled {
                    "cancelled"
                } else {
                    "operator_stop"
                };
            evidence.wal = Some(wal);
            return;
        }
        if number > budget.max_wal_frames {
            evidence.state = "partial";
            evidence.coverage.reason = "budget";
            evidence.wal = Some(wal);
            return;
        }
        if wal.frames.try_reserve(1).is_err() {
            evidence.state = "partial";
            evidence.coverage.reason = "allocation_failure";
            evidence.wal = Some(wal);
            return;
        }
        let frame = read_frame(file, &mut buffer, offset, number, &mut header);
        evidence.evaluated(offset + u64::from(frame.readable_bytes));
        if let Some(code) = frame.diagnostic {
            if frame.state == "unresolved" {
                evidence.state = "unresolved_tail";
                evidence.coverage.reason = "salt_boundary";
            }
            evidence.diagnostic(code, offset, u64::from(frame.readable_bytes));
            wal.frames.push(frame);
            // Later frames depend on this rolling checksum/salt boundary.
            evidence.wal = Some(wal);
            return;
        }
        wal.validated_frames = number.to_string();
        if frame.database_size != Some(0) {
            wal.last_commit_frame = Some(number.to_string());
        }
        wal.frames.push(frame);
        offset += buffer.len() as u64;
        number += 1;
    }
    evidence.state = if size_matches {
        "validated"
    } else {
        "conflicting"
    };
    evidence.coverage.reason = "complete";
    evidence.wal = Some(wal);
}
