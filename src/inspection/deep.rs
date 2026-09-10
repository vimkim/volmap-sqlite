//! Bounded reconstruction and typed decoding for one explicitly selected cell.
use std::fs::File;
use std::os::unix::fs::FileExt;

use serde::{Deserialize, Serialize};

use super::{CellIdentity, InspectionGraph, PageIdentity, PhysicalEvidence, TextEncoding};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeepSelector {
    pub session_id: String,
    pub snapshot_id: String,
    pub revision: u64,
    pub page_number: u32,
    pub cell_index: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeepBudget {
    pub max_payload_bytes: u64,
    pub max_overflow_pages: u32,
    pub max_values: u32,
    #[serde(default = "default_decoded_bytes")]
    pub max_decoded_bytes: u64,
}

fn default_decoded_bytes() -> u64 {
    16 * 1024 * 1024
}

impl Default for DeepBudget {
    fn default() -> Self {
        Self {
            max_payload_bytes: 16 * 1024 * 1024,
            max_overflow_pages: 32_768,
            max_values: 4096,
            max_decoded_bytes: default_decoded_bytes(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeepState {
    Pending,
    Completed,
    Cancelled,
    BudgetStopped,
    InvalidTarget,
    StaleRevision,
    InvalidatedSnapshot,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepCoverage {
    pub phase: &'static str,
    pub reason: &'static str,
    pub payload_bytes: Option<String>,
    pub reconstructed_bytes: String,
    pub remainder_bytes: Option<String>,
    pub decoded_values: u32,
    pub decoded_bytes: String,
    pub expected_values: Option<u32>,
    pub stopping_page: Option<u32>,
    pub stopping_payload_offset: Option<String>,
    pub evidence: Vec<PhysicalEvidence>,
    pub overflow_pages: Vec<PageIdentity>,
}

impl Default for DeepCoverage {
    fn default() -> Self {
        Self {
            phase: "queued",
            reason: "pending",
            payload_bytes: None,
            reconstructed_bytes: "0".into(),
            remainder_bytes: None,
            decoded_values: 0,
            decoded_bytes: "0".into(),
            expected_values: None,
            stopping_page: None,
            stopping_payload_offset: None,
            evidence: vec![],
            overflow_pages: vec![],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepStatus {
    pub id: String,
    pub target: DeepSelector,
    pub state: DeepState,
    pub budget: DeepBudget,
    pub coverage: DeepCoverage,
    pub result_revision: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepField {
    pub ordinal: u32,
    pub serial_type: String,
    pub payload_offset: String,
    pub byte_length: String,
    pub source: Vec<PhysicalEvidence>,
    pub column_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypedValue {
    Null,
    Integer {
        value: String,
    },
    Real {
        value: String,
    },
    Text {
        value: String,
    },
    #[serde(rename_all = "camelCase")]
    Blob {
        byte_length: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepValue {
    pub field: DeepField,
    pub value: TypedValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepEvidence {
    pub cell: CellIdentity,
    pub fields: Vec<DeepField>,
    pub coverage: DeepCoverage,
    pub column_metadata: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepResult {
    pub target: DeepSelector,
    pub revision: u64,
    pub evidence: DeepEvidence,
    pub values: Vec<DeepValue>,
}

pub(super) struct Decoded {
    pub evidence: DeepEvidence,
    pub values: Vec<DeepValue>,
}

pub(super) struct Failure {
    pub state: DeepState,
    pub coverage: Box<DeepCoverage>,
}

fn fail(state: DeepState, reason: &'static str, coverage: &DeepCoverage) -> Failure {
    let mut coverage = coverage.clone();
    coverage.reason = reason;
    Failure {
        state,
        coverage: Box::new(coverage),
    }
}

pub(super) struct Source<'a> {
    pub file: &'a File,
    pub graph: &'a InspectionGraph,
    pub pages: &'a super::storage::PageStore,
}

pub(super) fn inspect(
    source: &Source<'_>,
    target: &DeepSelector,
    budget: DeepBudget,
    checkpoint: &mut impl FnMut(&DeepCoverage) -> bool,
) -> Result<Decoded, Failure> {
    let Source { file, graph, pages } = source;
    let mut coverage = DeepCoverage {
        phase: "payload",
        ..DeepCoverage::default()
    };
    let page = pages
        .get_admitted(
            target.page_number.saturating_sub(1) as usize,
            graph.operational_budget.max_resident_bytes,
        )
        .map_err(|error| page_read_failure(error, &coverage))?
        .ok_or_else(|| fail(DeepState::InvalidTarget, "page_not_found", &coverage))?;
    let cell = page
        .detail
        .cells
        .get(usize::from(target.cell_index))
        .ok_or_else(|| fail(DeepState::InvalidTarget, "cell_not_found", &coverage))?;
    let local = cell
        .local_payload
        .as_ref()
        .ok_or_else(|| fail(DeepState::InvalidTarget, "cell_has_no_record", &coverage))?;
    let size = cell
        .payload_size
        .ok_or_else(|| fail(DeepState::InvalidTarget, "payload_unavailable", &coverage))?;
    coverage.payload_bytes = Some(size.to_string());
    coverage.remainder_bytes = Some(size.to_string());
    coverage.stopping_page = Some(target.page_number);
    coverage.stopping_payload_offset = Some("0".into());
    if !checkpoint(&coverage) {
        return Err(fail(DeepState::Cancelled, "cancelled", &coverage));
    }
    if size > budget.max_payload_bytes {
        return Err(fail(
            DeepState::BudgetStopped,
            "payload_byte_budget",
            &coverage,
        ));
    }
    let mut payload = Vec::new();
    let local_evidence = PhysicalEvidence {
        page: PageIdentity {
            page_number: target.page_number,
        },
        range: local.clone(),
        validation_rule: std::borrow::Cow::Borrowed("selected_cell_payload"),
    };
    read_segment(
        file,
        &local_evidence,
        &mut payload,
        &mut coverage,
        checkpoint,
    )?;
    if u64::from(local.length) < size {
        read_overflow(
            source,
            target,
            budget,
            &mut payload,
            &mut coverage,
            checkpoint,
        )?;
    }
    if payload.len() as u64 != size {
        return Err(fail(
            DeepState::Failed,
            "payload_length_mismatch",
            &coverage,
        ));
    }
    coverage.phase = "record";
    let values = decode_record(
        &payload,
        graph.snapshot.geometry.text_encoding,
        graph.snapshot.geometry.schema_format,
        budget,
        &mut coverage,
        checkpoint,
    )?;
    coverage.phase = "publication";
    coverage.reason = "complete";
    coverage.stopping_page = None;
    coverage.stopping_payload_offset = None;
    let fields = values.iter().map(|value| value.field.clone()).collect();
    Ok(Decoded {
        evidence: DeepEvidence {
            cell: cell.identity.clone(),
            fields,
            coverage,
            column_metadata: "unavailable",
        },
        values,
    })
}

fn page_read_failure(error: super::storage::StorageError, coverage: &DeepCoverage) -> Failure {
    match error {
        super::storage::StorageError::Budget => {
            fail(DeepState::BudgetStopped, "resident_memory_budget", coverage)
        }
        super::storage::StorageError::Unavailable => {
            fail(DeepState::Failed, "private_storage_unavailable", coverage)
        }
    }
}

fn decode_record(
    bytes: &[u8],
    encoding: TextEncoding,
    schema_format: u32,
    budget: DeepBudget,
    coverage: &mut DeepCoverage,
    checkpoint: &mut impl FnMut(&DeepCoverage) -> bool,
) -> Result<Vec<DeepValue>, Failure> {
    set_record_boundary(coverage, 0);
    let invalid = |coverage: &DeepCoverage| fail(DeepState::Failed, "record_invalid", coverage);
    if !(1..=4).contains(&schema_format) {
        return Err(invalid(coverage));
    }
    let mut cursor = 0;
    let header = super::btree::varint(bytes, &mut cursor)
        .ok()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|end| *end >= cursor && *end <= bytes.len())
        .ok_or_else(|| invalid(coverage))?;
    let mut serials = Vec::new();
    while cursor < header {
        set_record_boundary(coverage, cursor);
        if !checkpoint(coverage) {
            return Err(fail(DeepState::Cancelled, "cancelled", coverage));
        }
        if serials.len() >= budget.max_values as usize {
            return Err(fail(
                DeepState::BudgetStopped,
                "value_count_budget",
                coverage,
            ));
        }
        let serial =
            super::btree::varint(&bytes[..header], &mut cursor).map_err(|_| invalid(coverage))?;
        serials.push(serial);
    }
    coverage.expected_values = Some(u32::try_from(serials.len()).map_err(|_| invalid(coverage))?);
    let mut values = Vec::new();
    let mut decoded_bytes = 0_u64;
    for serial in serials {
        set_record_boundary(coverage, cursor);
        if !checkpoint(coverage) {
            return Err(fail(DeepState::Cancelled, "cancelled", coverage));
        }
        let length = match serial {
            8 | 9 if schema_format < 4 => return Err(invalid(coverage)),
            0 | 8 | 9 => 0,
            1..=4 => serial,
            5 => 6,
            6 | 7 => 8,
            10 | 11 => return Err(invalid(coverage)),
            _ => (serial - 12) / 2,
        };
        let length = usize::try_from(length).map_err(|_| invalid(coverage))?;
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid(coverage))?;
        let raw = &bytes[cursor..end];
        let text_length = if serial >= 13 && serial % 2 == 1 {
            decoded_text_length(raw, encoding)
                .map_err(|()| fail(DeepState::Failed, "text_encoding_invalid", coverage))?
        } else {
            0
        };
        if text_length > budget.max_decoded_bytes.saturating_sub(decoded_bytes) {
            return Err(fail(
                DeepState::BudgetStopped,
                "decoded_byte_budget",
                coverage,
            ));
        }
        let value = decode_value(serial, raw, encoding, coverage)?;
        let value_bytes = match &value {
            TypedValue::Null => 0,
            TypedValue::Integer { value }
            | TypedValue::Real { value }
            | TypedValue::Text { value } => value.len() as u64,
            TypedValue::Blob { byte_length } => byte_length.len() as u64,
        };
        if value_bytes > budget.max_decoded_bytes.saturating_sub(decoded_bytes) {
            return Err(fail(
                DeepState::BudgetStopped,
                "decoded_byte_budget",
                coverage,
            ));
        }
        decoded_bytes += value_bytes;
        coverage.decoded_bytes = decoded_bytes.to_string();
        let field = DeepField {
            ordinal: coverage.decoded_values,
            serial_type: serial.to_string(),
            payload_offset: cursor.to_string(),
            byte_length: length.to_string(),
            source: field_source(cursor, length, &coverage.evidence),
            column_name: None,
        };
        values.push(DeepValue { field, value });
        coverage.decoded_values += 1;
        cursor = end;
    }
    if cursor != bytes.len() {
        set_record_boundary(coverage, cursor);
        return Err(invalid(coverage));
    }
    Ok(values)
}

fn decode_value(
    serial: u64,
    raw: &[u8],
    encoding: TextEncoding,
    coverage: &DeepCoverage,
) -> Result<TypedValue, Failure> {
    let invalid = |coverage: &DeepCoverage| fail(DeepState::Failed, "record_invalid", coverage);
    let length = raw.len();
    Ok(match serial {
        0 => TypedValue::Null,
        8 | 9 => TypedValue::Integer {
            value: (serial - 8).to_string(),
        },
        1..=6 => {
            let mut signed = [if raw[0] & 128 == 0 { 0 } else { 255 }; 8];
            signed[8 - raw.len()..].copy_from_slice(raw);
            TypedValue::Integer {
                value: i64::from_be_bytes(signed).to_string(),
            }
        }
        7 => TypedValue::Real {
            value: f64::from_be_bytes(raw.try_into().map_err(|_| invalid(coverage))?).to_string(),
        },
        serial if serial % 2 == 0 => TypedValue::Blob {
            byte_length: length.to_string(),
        },
        _ => TypedValue::Text {
            value: super::schema::decode_text(raw, encoding)
                .map_err(|_| fail(DeepState::Failed, "text_encoding_invalid", coverage))?,
        },
    })
}

// Count UTF-8 output before allocating a decoded string, including UTF-16 expansion.
fn decoded_text_length(raw: &[u8], encoding: TextEncoding) -> Result<u64, ()> {
    if encoding == TextEncoding::Utf8 {
        return std::str::from_utf8(raw)
            .map(|s| s.len() as u64)
            .map_err(|_| ());
    }
    if !raw.len().is_multiple_of(2) {
        return Err(());
    }
    let units = raw.chunks_exact(2).map(|pair| {
        let pair = [pair[0], pair[1]];
        if encoding == TextEncoding::Utf16Le {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    });
    char::decode_utf16(units).try_fold(0, |n, ch| {
        ch.map(|ch| n + ch.len_utf8() as u64).map_err(|_| ())
    })
}

fn field_source(
    offset: usize,
    length: usize,
    segments: &[PhysicalEvidence],
) -> Vec<PhysicalEvidence> {
    let mut start = 0;
    segments
        .iter()
        .filter_map(|segment| {
            let end = start + segment.range.length as usize;
            let overlap_start = offset.max(start);
            let overlap_end = (offset + length).min(end);
            let source = (overlap_start < overlap_end).then(|| {
                let mut source = segment.clone();
                let delta = u32::try_from(overlap_start - start).expect("bounded page extent");
                source.range.page_offset += delta;
                source.range.file_offset += u64::from(delta);
                source.range.length =
                    u32::try_from(overlap_end - overlap_start).expect("bounded page extent");
                source.validation_rule = std::borrow::Cow::Borrowed("selected_record_field");
                source
            });
            start = end;
            source
        })
        .collect()
}

fn read_segment(
    file: &File,
    source: &PhysicalEvidence,
    payload: &mut Vec<u8>,
    coverage: &mut DeepCoverage,
    checkpoint: &mut impl FnMut(&DeepCoverage) -> bool,
) -> Result<(), Failure> {
    let mut offset = 0;
    while offset < source.range.length {
        coverage.stopping_page = Some(source.page.page_number);
        coverage.stopping_payload_offset = Some(payload.len().to_string());
        if !checkpoint(coverage) {
            return Err(fail(DeepState::Cancelled, "cancelled", coverage));
        }
        let length = (source.range.length - offset).min(16 * 1024);
        let start = payload.len();
        payload
            .try_reserve_exact(length as usize)
            .map_err(|_| fail(DeepState::BudgetStopped, "allocation_limit", coverage))?;
        payload.resize(start + length as usize, 0);
        file.read_exact_at(
            &mut payload[start..],
            source.range.file_offset + u64::from(offset),
        )
        .map_err(|_| fail(DeepState::Failed, "payload_read_failed", coverage))?;
        let mut evidence = source.clone();
        evidence.range.page_offset += offset;
        evidence.range.file_offset += u64::from(offset);
        evidence.range.length = length;
        coverage.evidence.push(evidence);
        coverage.reconstructed_bytes = payload.len().to_string();
        let total = coverage
            .payload_bytes
            .as_deref()
            .and_then(|size| size.parse::<u64>().ok())
            .unwrap_or(0);
        coverage.remainder_bytes = Some(total.saturating_sub(payload.len() as u64).to_string());
        offset += length;
        coverage.stopping_payload_offset = Some(payload.len().to_string());
    }
    Ok(())
}

fn read_overflow(
    source: &Source<'_>,
    target: &DeepSelector,
    budget: DeepBudget,
    payload: &mut Vec<u8>,
    coverage: &mut DeepCoverage,
    checkpoint: &mut impl FnMut(&DeepCoverage) -> bool,
) -> Result<(), Failure> {
    use super::{EntityIdentity, PageRole, TraversalStopReason};
    let Source { file, graph, pages } = source;
    let mut source = EntityIdentity::Cell {
        page_number: target.page_number,
        cell_index: target.cell_index,
    };
    let traversal = selected_overflow(
        pages,
        &source,
        graph.operational_budget.max_resident_bytes,
        coverage,
        checkpoint,
    )?;
    let total = coverage
        .payload_bytes
        .as_deref()
        .and_then(|size| size.parse::<u64>().ok())
        .unwrap_or(0);
    let links = overflow_links(
        pages,
        &traversal,
        budget.max_overflow_pages,
        coverage,
        checkpoint,
    )?;
    for page in &traversal.validated_prefix {
        coverage.stopping_page = Some(page.page_number);
        coverage.stopping_payload_offset = Some(payload.len().to_string());
        if !checkpoint(coverage) {
            return Err(fail(DeepState::Cancelled, "cancelled", coverage));
        }
        if coverage.overflow_pages.len() >= budget.max_overflow_pages as usize {
            return Err(fail(
                DeepState::BudgetStopped,
                "overflow_page_budget",
                coverage,
            ));
        }
        let valid_role = pages
            .get_admitted(
                page.page_number
                    .checked_sub(1)
                    .map_or(usize::MAX, |number| number as usize),
                graph.operational_budget.max_resident_bytes,
            )
            .map_err(|error| page_read_failure(error, coverage))?
            .is_some_and(|page| {
                page.classification.reconciled && page.classification.role == PageRole::Overflow
            });
        let valid_link = links.contains(&overflow_link_key(&source, page.page_number));
        if !valid_role || !valid_link {
            return Err(fail(
                DeepState::Failed,
                "overflow_relationship_unavailable",
                coverage,
            ));
        }
        let length = total
            .saturating_sub(payload.len() as u64)
            .min(u64::from(graph.snapshot.geometry.usable_size - 4));
        if length == 0 {
            return Err(fail(
                DeepState::Failed,
                "overflow_length_mismatch",
                coverage,
            ));
        }
        let evidence = super::roles::evidence(
            &graph.snapshot.geometry,
            page.page_number,
            4,
            u32::try_from(length).expect("bounded overflow page"),
            "selected_overflow_payload",
        );
        read_segment(file, &evidence, payload, coverage, checkpoint)?;
        coverage.overflow_pages.push(page.clone());
        source = EntityIdentity::Page {
            page_number: page.page_number,
        };
    }
    if let Some(stop) = &traversal.stop {
        coverage.stopping_page = stop.intended_target.as_ref().map(|page| page.page_number);
        coverage.stopping_payload_offset = Some(payload.len().to_string());
        let state = if matches!(
            stop.reason,
            TraversalStopReason::Budget | TraversalStopReason::CoverageStop
        ) {
            DeepState::BudgetStopped
        } else {
            DeepState::Failed
        };
        return Err(fail(state, "overflow_prefix_stopped", coverage));
    }
    Ok(())
}

type OverflowLink = (u32, Option<u16>, u32);

fn selected_overflow(
    pages: &super::storage::PageStore,
    source: &super::EntityIdentity,
    ceiling: u64,
    coverage: &DeepCoverage,
    checkpoint: &mut impl FnMut(&DeepCoverage) -> bool,
) -> Result<super::Traversal, Failure> {
    let page = overflow_link_key(source, 0).0;
    let mut start = 0;
    while let Some(position) = pages
        .traversals
        .page_position(page, start)
        .map_err(|_| fail(DeepState::Failed, "private_storage_unavailable", coverage))?
    {
        if !checkpoint(coverage) {
            return Err(fail(DeepState::Cancelled, "cancelled", coverage));
        }
        let reservation = pages
            .traversals
            .estimated_size(position)
            .map_err(|_| fail(DeepState::Failed, "private_storage_unavailable", coverage))?;
        if !super::budget::memory_available(ceiling, reservation) {
            return Err(fail(
                DeepState::BudgetStopped,
                "resident_memory_budget",
                coverage,
            ));
        }
        let traversal = pages
            .traversals
            .get(position)
            .ok_or_else(|| fail(DeepState::Failed, "private_storage_unavailable", coverage))?;
        if traversal.kind == super::TraversalKind::Overflow && traversal.origin == *source {
            return Ok(traversal);
        }
        start = position + 1;
    }
    Err(fail(DeepState::Failed, "overflow_unavailable", coverage))
}

fn overflow_link_key(source: &super::EntityIdentity, target: u32) -> OverflowLink {
    match source {
        super::EntityIdentity::Page { page_number } => (*page_number, None, target),
        super::EntityIdentity::Cell {
            page_number,
            cell_index,
        } => (*page_number, Some(*cell_index), target),
    }
}

fn overflow_links(
    pages: &super::storage::PageStore,
    traversal: &super::Traversal,
    max_pages: u32,
    coverage: &DeepCoverage,
    checkpoint: &mut impl FnMut(&DeepCoverage) -> bool,
) -> Result<std::collections::HashSet<OverflowLink>, Failure> {
    let mut source = traversal.origin.clone();
    let mut links = std::collections::HashSet::new();
    for page in traversal.validated_prefix.iter().take(max_pages as usize) {
        let wanted = overflow_link_key(&source, page.page_number);
        let mut start = 0;
        while let Some(position) = pages
            .relationships
            .page_position(wanted.0, start)
            .map_err(|_| fail(DeepState::Failed, "private_storage_unavailable", coverage))?
        {
            if !checkpoint(coverage) {
                return Err(fail(DeepState::Cancelled, "cancelled", coverage));
            }
            let link = pages
                .relationships
                .get(position)
                .ok_or_else(|| fail(DeepState::Failed, "private_storage_unavailable", coverage))?;
            if link.kind == super::RelationshipKind::Overflow
                && overflow_link_key(&link.source, link.target.page_number) == wanted
            {
                links.insert(wanted);
                break;
            }
            start = position + 1;
        }
        source = super::EntityIdentity::Page {
            page_number: page.page_number,
        };
    }
    pages
        .check()
        .map_err(|_| fail(DeepState::Failed, "private_storage_unavailable", coverage))?;
    Ok(links)
}

fn set_record_boundary(coverage: &mut DeepCoverage, offset: usize) {
    coverage.stopping_payload_offset = Some(offset.to_string());
    let mut start = 0;
    coverage.stopping_page = coverage.evidence.iter().find_map(|segment| {
        let end = start + segment.range.length as usize;
        let contains = offset >= start && offset < end;
        start = end;
        contains.then_some(segment.page.page_number)
    });
}
