use std::fs::File;
use std::os::unix::fs::FileExt;

use serde::Serialize;

use super::DatabaseGeometry;

/// Half-open byte extent, expressed in page-relative and main-file coordinates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteRange {
    pub page_offset: u32,
    pub file_offset: u64,
    pub length: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub kind: &'static str,
    pub range: ByteRange,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BtreeHeader {
    pub range: ByteRange,
    pub first_freeblock: u32,
    pub cell_count: u16,
    pub content_start: u32,
    pub fragmented_bytes: u8,
    pub rightmost_child: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellIdentity {
    pub page_number: u32,
    /// Zero-based physical cell-pointer-array index, not a decoded key.
    pub index: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDetail {
    pub state: RecordState,
    pub header_size: Option<u64>,
    /// Decimal strings preserve all 64 bits across JSON/JavaScript.
    pub serial_types: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    Complete,
    Invalid,
    NeedsOverflow,
    UnsupportedFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalCoverage {
    Complete,
    Partial,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellDetail {
    pub identity: CellIdentity,
    pub pointer: ByteRange,
    pub offset: u32,
    pub range: Option<ByteRange>,
    pub rowid: Option<String>,
    pub left_child: Option<u32>,
    pub payload_size: Option<u64>,
    pub local_payload: Option<ByteRange>,
    pub overflow_page: Option<u32>,
    pub record: Option<RecordDetail>,
    pub diagnostic: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Freeblock {
    pub offset: u32,
    pub next: u32,
    pub range: Option<ByteRange>,
    pub diagnostic: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDetail {
    /// A locally validated B-tree header claim; global role reconciliation is separate.
    pub kind: Option<Kind>,
    pub header: Option<BtreeHeader>,
    pub regions: Vec<Region>,
    pub cells: Vec<CellDetail>,
    pub freeblocks: Vec<Freeblock>,
    pub diagnostics: Vec<&'static str>,
    pub coverage: LocalCoverage,
}

impl PageDetail {
    fn finish_coverage(&mut self) {
        self.coverage = if !self.diagnostics.is_empty()
            || self
                .freeblocks
                .iter()
                .any(|block| block.diagnostic.is_some())
            || self.cells.iter().any(|cell| {
                cell.diagnostic.is_some()
                    || cell
                        .record
                        .as_ref()
                        .is_some_and(|record| record.state != RecordState::Complete)
            }) {
            LocalCoverage::Partial
        } else {
            LocalCoverage::Complete
        };
    }
}

struct Page<'a> {
    bytes: &'a [u8],
    number: u32,
    size: u32,
    usable: usize,
    schema_format: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    TableLeaf,
    TableInterior,
    IndexLeaf,
    IndexInterior,
}

impl Kind {
    fn interior(self) -> bool {
        matches!(self, Self::TableInterior | Self::IndexInterior)
    }
}

impl Page<'_> {
    fn range(&self, start: usize, length: usize) -> ByteRange {
        ByteRange {
            page_offset: u32::try_from(start).expect("page-bounded offset"),
            file_offset: u64::from(self.number - 1) * u64::from(self.size) + start as u64,
            length: u32::try_from(length).expect("page-bounded length"),
        }
    }

    fn region(&self, kind: &'static str, start: usize, end: usize) -> Region {
        Region {
            kind,
            range: self.range(start, end - start),
        }
    }

    fn inspect(&self) -> PageDetail {
        let base = if self.number == 1 { 100 } else { 0 };
        let mut detail = PageDetail {
            kind: None,
            header: None,
            regions: vec![],
            cells: vec![],
            freeblocks: vec![],
            diagnostics: vec![],
            coverage: LocalCoverage::Unsupported,
        };
        if base != 0 {
            detail.regions.push(self.region("database_header", 0, base));
        }
        detail
            .regions
            .push(self.region("usable_space", 0, self.usable));
        detail
            .regions
            .push(self.region("opaque_reserved", self.usable, self.bytes.len()));
        let kind = match self.bytes[base] {
            13 => Kind::TableLeaf,
            5 => Kind::TableInterior,
            10 => Kind::IndexLeaf,
            2 => Kind::IndexInterior,
            _ => return detail,
        };
        if self.number == 1 && matches!(kind, Kind::IndexLeaf | Kind::IndexInterior) {
            detail.diagnostics.push("invalid_page_one_kind");
            detail.coverage = LocalCoverage::Partial;
            return detail;
        }
        let header_size = if kind.interior() { 12 } else { 8 };
        let count = word(self.bytes, base + 3);
        let encoded_start = word(self.bytes, base + 5);
        let start = if encoded_start == 0 {
            65_536
        } else {
            usize::from(encoded_start)
        };
        let pointer_end = base + header_size + usize::from(count) * 2;
        if pointer_end > start || start > self.usable || self.bytes[base + 7] > 60 {
            detail.diagnostics.push("invalid_btree_header");
            detail.coverage = LocalCoverage::Partial;
            return detail;
        }
        detail.kind = Some(kind);
        detail.header = Some(BtreeHeader {
            range: self.range(base, header_size),
            first_freeblock: u32::from(word(self.bytes, base + 1)),
            cell_count: count,
            content_start: u32::try_from(start).unwrap(),
            fragmented_bytes: self.bytes[base + 7],
            rightmost_child: kind.interior().then(|| dword(self.bytes, base + 8)),
        });
        detail.regions.extend([
            self.region("btree_header", base, base + header_size),
            self.region("cell_pointer_array", base + header_size, pointer_end),
            self.region("unallocated", pointer_end, start),
            self.region("cell_content", start, self.usable),
        ]);
        for index in 0..count {
            let pointer = base + header_size + usize::from(index) * 2;
            let offset = usize::from(word(self.bytes, pointer));
            let mut cell = CellDetail {
                identity: CellIdentity {
                    page_number: self.number,
                    index,
                },
                pointer: self.range(pointer, 2),
                offset: u32::try_from(offset).unwrap(),
                range: None,
                rowid: None,
                record: None,
                diagnostic: None,
                left_child: None,
                payload_size: None,
                local_payload: None,
                overflow_page: None,
            };
            let result = if offset < start || offset >= self.usable {
                Err("invalid_cell_pointer")
            } else {
                self.cell(kind, offset, &mut cell)
            };
            cell.diagnostic = result.err();
            detail.cells.push(cell);
        }
        self.freeblocks(&mut detail, start, usize::from(word(self.bytes, base + 1)));
        contain_overlaps(&mut detail, start, self.usable);
        self.records(&mut detail);
        self.fragments(&mut detail, start, self.bytes[base + 7]);
        detail.finish_coverage();
        detail
    }

    fn freeblocks(&self, detail: &mut PageDetail, start: usize, mut offset: usize) {
        // Prefix maxima allow overlap queries without quadratic scans of cells.
        let mut occupied: Vec<_> = detail
            .cells
            .iter()
            .map(|cell| {
                (
                    cell.offset,
                    cell.range
                        .as_ref()
                        .map_or(cell.offset + 1, |range| range.page_offset + range.length),
                )
            })
            .collect();
        occupied.sort_unstable();
        let mut high = 0;
        for (_, end) in &mut occupied {
            high = high.max(*end);
            *end = high;
        }
        while offset != 0 {
            if offset < start || offset + 4 > self.usable {
                detail.diagnostics.push("invalid_freeblock_extent");
                break;
            }
            let next = usize::from(word(self.bytes, offset));
            let size = usize::from(word(self.bytes, offset + 2));
            if size < 4 || offset + size > self.usable {
                detail.diagnostics.push("invalid_freeblock_extent");
                break;
            }
            detail.freeblocks.push(Freeblock {
                offset: u32::try_from(offset).unwrap(),
                next: u32::try_from(next).unwrap(),
                range: Some(self.range(offset, size)),
                diagnostic: None,
            });
            let before_end =
                occupied.partition_point(|(start, _)| (*start as usize) < offset + size);
            if before_end > 0 && occupied[before_end - 1].1 as usize > offset {
                // The link is inside a disputed allocation; it cannot authorize a successor.
                break;
            }
            if detail.freeblocks.len() == 1
                && !detail.cells.iter().any(|cell| {
                    cell.range
                        .as_ref()
                        .is_some_and(|range| (range.page_offset as usize) < offset)
                })
            {
                detail.diagnostics.push("freeblock_without_preceding_cell");
                detail.freeblocks[0].range = None;
                detail.freeblocks[0].diagnostic = Some("freeblock_without_preceding_cell");
                break;
            }
            if next != 0 && next < offset + size {
                detail.diagnostics.push("invalid_freeblock_link");
                break;
            }
            offset = next;
        }
    }

    fn records(&self, detail: &mut PageDetail) {
        // Record headers are interpreted only after allocations are disjoint.
        // This also bounds total local record work to the usable page size.
        for cell in &mut detail.cells {
            if let (Some(local), Some(payload)) = (&cell.local_payload, cell.payload_size) {
                let start = local.page_offset as usize;
                let record = record(
                    &self.bytes[start..start + local.length as usize],
                    payload,
                    self.schema_format,
                );
                if record.state == RecordState::Invalid {
                    cell.diagnostic = Some("invalid_record");
                }
                cell.record = Some(record);
            }
        }
    }

    fn fragments(&self, detail: &mut PageDetail, start: usize, declared: u8) {
        // Gaps are only meaningful when every allocation boundary is known.
        if !detail.diagnostics.is_empty()
            || detail.cells.iter().any(|cell| cell.range.is_none())
            || detail.freeblocks.iter().any(|block| block.range.is_none())
        {
            return;
        }
        let mut ranges: Vec<_> = detail
            .cells
            .iter()
            .filter_map(|cell| cell.range.as_ref())
            .chain(
                detail
                    .freeblocks
                    .iter()
                    .filter_map(|block| block.range.as_ref()),
            )
            .collect();
        ranges.sort_unstable_by_key(|range| range.page_offset);
        let mut cursor = start;
        let mut fragments = vec![];
        for range in ranges {
            let end = range.page_offset as usize;
            if cursor < end {
                fragments.push((cursor, end));
            }
            cursor = (range.page_offset + range.length) as usize;
        }
        if cursor < self.usable {
            fragments.push((cursor, self.usable));
        }
        if fragments.iter().any(|(start, end)| end - start > 3) {
            detail.diagnostics.push("untracked_free_space");
            return;
        }
        let total: usize = fragments.iter().map(|(start, end)| end - start).sum();
        if total != usize::from(declared) {
            detail.diagnostics.push("fragment_count_mismatch");
        }
        detail.regions.extend(
            fragments
                .into_iter()
                .map(|(start, end)| self.region("fragment", start, end)),
        );
    }

    fn cell(&self, kind: Kind, offset: usize, cell: &mut CellDetail) -> Result<(), &'static str> {
        let mut cursor = offset;
        let left_child = if kind.interior() {
            if cursor + 4 > self.usable {
                return Err("invalid_cell_extent");
            }
            let child = dword(self.bytes, cursor);
            cursor += 4;
            Some(child)
        } else {
            None
        };
        if matches!(kind, Kind::TableInterior) {
            let rowid = varint(&self.bytes[..self.usable], &mut cursor)?;
            cell.range = Some(self.range(offset, cursor - offset));
            cell.rowid = Some(i64::from_be_bytes(rowid.to_be_bytes()).to_string());
            cell.left_child = left_child;
            return Ok(());
        }
        let payload = varint(&self.bytes[..self.usable], &mut cursor)?;
        if payload > 2_147_483_647 {
            return Err("invalid_payload_size");
        }
        let rowid = if matches!(kind, Kind::TableLeaf) {
            Some(varint(&self.bytes[..self.usable], &mut cursor)?)
        } else {
            None
        };
        // SQLite file format, section 1.7: minimum, maximum and modulo-local payload.
        let usable = self.usable as u64;
        let minimum = ((usable - 12) * 32 / 255) - 23;
        let maximum = if matches!(kind, Kind::TableLeaf) {
            usable - 35
        } else {
            ((usable - 12) * 64 / 255) - 23
        };
        let local = if payload <= maximum {
            payload
        } else {
            let candidate = minimum + (payload - minimum) % (usable - 4);
            if candidate <= maximum {
                candidate
            } else {
                minimum
            }
        };
        let local = usize::try_from(local).map_err(|_| "invalid_cell_extent")?;
        let spill = payload > local as u64;
        let end = cursor
            .checked_add(local + if spill { 4 } else { 0 })
            .filter(|end| *end <= self.usable)
            .ok_or("invalid_cell_extent")?;
        cell.range = Some(self.range(offset, end - offset));
        cell.rowid = rowid.map(|rowid| i64::from_be_bytes(rowid.to_be_bytes()).to_string());
        cell.left_child = left_child;
        cell.payload_size = Some(payload);
        cell.local_payload = Some(self.range(cursor, local));
        cell.overflow_page = spill.then(|| dword(self.bytes, cursor + local));
        Ok(())
    }
}

// Sweep sorted extents in O(n log n); even nested/duplicate extents join the
// same overlap component. Never publish interpreted facts from conflicting cells.
fn contain_overlaps(detail: &mut PageDetail, content_start: usize, usable: usize) {
    let cells = &mut detail.cells;
    let mut extents: Vec<_> = cells
        .iter()
        .enumerate()
        .filter_map(|(index, cell)| {
            if (cell.offset as usize) < content_start || cell.offset as usize >= usable {
                return None;
            }
            // Even an undecodable cell's start cannot be inside another allocation.
            Some((
                cell.offset,
                cell.range
                    .as_ref()
                    .map_or(cell.offset + 1, |range| range.page_offset + range.length),
                index,
            ))
        })
        .collect();
    extents.extend(
        detail
            .freeblocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                block.range.as_ref().map(|range| {
                    (
                        range.page_offset,
                        range.page_offset + range.length,
                        cells.len() + index,
                    )
                })
            }),
    );
    extents.sort_unstable();
    let mut start = 0;
    while start < extents.len() {
        let mut end = start + 1;
        let mut high = extents[start].1;
        while end < extents.len() && extents[end].0 < high {
            high = high.max(extents[end].1);
            end += 1;
        }
        if end > start + 1 {
            for &(_, _, index) in &extents[start..end] {
                if index >= cells.len() {
                    let block = &mut detail.freeblocks[index - cells.len()];
                    block.range = None;
                    block.diagnostic = Some("overlapping_allocation");
                    continue;
                }
                let cell = &mut cells[index];
                cell.range = None;
                cell.rowid = None;
                cell.record = None;
                cell.left_child = None;
                cell.payload_size = None;
                cell.local_payload = None;
                cell.overflow_page = None;
                cell.diagnostic = Some("overlapping_allocation");
            }
        }
        start = end;
    }
}

fn word(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("bounded four-byte field"),
    )
}

fn varint(bytes: &[u8], cursor: &mut usize) -> Result<u64, &'static str> {
    let mut value = 0_u64;
    for index in 0..9 {
        let byte = *bytes.get(*cursor).ok_or("truncated_varint")?;
        *cursor += 1;
        if index == 8 {
            return Ok((value << 8) | u64::from(byte));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    unreachable!()
}

fn record(bytes: &[u8], payload: u64, schema_format: u32) -> RecordDetail {
    let mut detail = RecordDetail {
        state: RecordState::Invalid,
        header_size: None,
        serial_types: vec![],
    };
    let mut cursor = 0;
    if !(1..=4).contains(&schema_format) {
        detail.state = RecordState::UnsupportedFormat;
        return detail;
    }
    let Ok(header) = varint(bytes, &mut cursor) else {
        return detail;
    };
    detail.header_size = Some(header);
    let Ok(end) = usize::try_from(header) else {
        return detail;
    };
    if end < cursor || header > payload {
        return detail;
    }
    let available = end.min(bytes.len());
    let mut body = 0_u64;
    while cursor < available {
        let Ok(serial) = varint(&bytes[..available], &mut cursor) else {
            if end > available {
                detail.state = RecordState::NeedsOverflow;
            }
            return detail;
        };
        let size = match serial {
            8 | 9 if schema_format < 4 => return detail,
            0 | 8 | 9 => 0,
            1..=4 => serial,
            5 => 6,
            6 | 7 => 8,
            10 | 11 => return detail,
            _ => (serial - 12) / 2,
        };
        let Some(next) = body.checked_add(size) else {
            return detail;
        };
        body = next;
        if header.checked_add(body).is_none_or(|total| total > payload) {
            return detail;
        }
        detail.serial_types.push(serial.to_string());
    }
    if end > available {
        detail.state = RecordState::NeedsOverflow;
    } else if header.checked_add(body) == Some(payload) {
        detail.state = RecordState::Complete;
    }
    detail
}

pub(super) fn read_page(file: &File, number: u32, geometry: &DatabaseGeometry) -> PageDetail {
    let mut bytes = vec![0; geometry.page_size as usize];
    let offset = u64::from(number - 1) * u64::from(geometry.page_size);
    if file.read_exact_at(&mut bytes, offset).is_err() {
        return PageDetail {
            kind: None,
            header: None,
            regions: vec![],
            cells: vec![],
            freeblocks: vec![],
            diagnostics: vec!["page_read_failed"],
            coverage: LocalCoverage::Partial,
        };
    }
    Page {
        bytes: &bytes,
        number,
        size: geometry.page_size,
        usable: geometry.usable_size as usize,
        schema_format: geometry.schema_format,
    }
    .inspect()
}
