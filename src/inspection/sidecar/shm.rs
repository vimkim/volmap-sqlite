use super::{SidecarEvidence, checksum, page_size_valid, read, word};
use serde::Serialize;
use std::fs::File;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShmEvidence {
    pub byte_order: &'static str,
    pub copies_match: Option<bool>,
    pub header_checksums_valid: Option<bool>,
    pub max_frame: Option<u32>,
    pub backfill: Option<u32>,
}

pub(super) fn inspect(file: &File, evidence: &mut SidecarEvidence, main_page_size: u32) {
    let mut header = [0; 136];
    let count = read(file, &mut header, 0);
    let big = cfg!(target_endian = "big");
    evidence.evaluated(count as u64);
    evidence.coverage.scope = "shm_header";
    evidence.coverage.reason = "validation_stop";
    evidence.state = "malformed";
    let names = [
        ("version", 0),
        ("change_counter", 8),
        ("max_frame", 16),
        ("database_pages", 20),
        ("frame_checksum_1", 24),
        ("frame_checksum_2", 28),
        ("header_checksum_1", 40),
        ("header_checksum_2", 44),
    ];
    for base in [0, 48] {
        if count >= base {
            evidence.fields(
                &header[base..count.min(base + 48)],
                &names,
                base as u64,
                big,
            );
            // Salt bytes are copied verbatim from the big-endian WAL header.
            evidence.fields(
                &header[base..count.min(base + 48)],
                &[("salt_1", 32), ("salt_2", 36)],
                base as u64,
                true,
            );
        }
    }
    evidence.fields(
        &header[..count],
        &[
            ("backfill", 96),
            ("read_mark_0", 100),
            ("read_mark_1", 104),
            ("read_mark_2", 108),
            ("read_mark_3", 112),
            ("read_mark_4", 116),
            ("backfill_attempted", 128),
        ],
        0,
        big,
    );
    let copies = (count >= 96).then(|| header[..48] == header[48..96]);
    let sums = (count >= 96).then(|| {
        [0, 48].into_iter().all(|base| {
            checksum(&header[base..base + 40], big, [0, 0])
                == [word(&header, base + 40, big), word(&header, base + 44, big)]
        })
    });
    evidence.shm = Some(ShmEvidence {
        byte_order: if big { "native_big" } else { "native_little" },
        copies_match: copies,
        header_checksums_valid: sums,
        max_frame: (count >= 20).then(|| word(&header, 16, big)),
        backfill: (count >= 100).then(|| word(&header, 96, big)),
    });
    validate(&header, count, evidence, main_page_size, copies, sums);
}

fn validate(
    header: &[u8; 136],
    count: usize,
    evidence: &mut SidecarEvidence,
    main_page_size: u32,
    copies: Option<bool>,
    sums: Option<bool>,
) {
    let big = cfg!(target_endian = "big");
    if count < 136 {
        evidence.diagnostic("shm_header_truncated", 0, count as u64);
        return;
    }
    if word(header, 0, big) != 3_007_000 {
        evidence.state = "unsupported";
        evidence.diagnostic("shm_native_version", 0, 4);
        return;
    }
    if copies != Some(true) {
        evidence.diagnostic("shm_header_copies", 0, 96);
    }
    if sums != Some(true) {
        evidence.diagnostic("shm_header_checksum", 0, 96);
    }
    if header[12] != 1 || header[13] > 1 {
        evidence.diagnostic("shm_initialization", 12, 2);
    }
    let raw = if big {
        u16::from_be_bytes([header[14], header[15]])
    } else {
        u16::from_le_bytes([header[14], header[15]])
    };
    let size = if raw == 1 { 65536 } else { u32::from(raw) };
    evidence.fields.push(super::SidecarField {
        name: "page_size",
        value: size,
        offset: "14".into(),
        length: 2,
    });
    if !page_size_valid(size) {
        evidence.diagnostic("shm_page_size", 14, 2);
    } else if size != main_page_size {
        evidence.diagnostic("shm_main_page_size_mismatch", 14, 2);
    }
    if word(header, 96, big) > word(header, 16, big) {
        evidence.diagnostic("shm_backfill_exceeds_frames", 96, 4);
    }
    let length = evidence.length.parse::<u64>().expect("file length");
    if !length.is_multiple_of(32768) {
        evidence.diagnostic("shm_file_extent", 0, length);
    }
    if evidence.diagnostics.is_empty() {
        evidence.state = "supported_header";
        evidence.coverage.reason = "supported_scope_complete";
    }
}
