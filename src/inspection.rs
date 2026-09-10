use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

use serde::Serialize;
use thiserror::Error;

mod budget;
pub use budget::OperationalBudget;
mod btree;
mod deep;
pub use deep::{
    DeepBudget, DeepCoverage, DeepEvidence, DeepField, DeepResult, DeepSelector, DeepState,
    DeepStatus, DeepValue, TypedValue,
};
mod pointer_map;
mod roles;
pub use pointer_map::{
    PointerMapEntry, PointerMapEvidence, PointerMapKind, PointerMapLayout, PointerMapPage,
};
pub use roles::{PageClassification, PageRole, RoleClaim};
mod freelist;
pub use freelist::{
    AllocationRole, FreelistCoverage, FreelistCoverageReason, FreelistEvidence, FreelistField,
    FreelistTrunk,
};
mod sidecar;
pub use sidecar::{
    JournalEvidence, ShmEvidence, SidecarBudget, SidecarCoverage, SidecarDiagnostic,
    SidecarEvidence, SidecarField, WalEvidence, WalFrame,
};
mod schema;
pub use schema::{SchemaBudget, SchemaEvidence, SchemaObject, SchemaObjectType, SchemaState};
mod session;
mod topology;
pub use btree::{
    ByteRange, CellDetail, CellIdentity, Kind as BtreeKind, LocalCoverage, PageDetail, RecordState,
};
pub use session::{
    Coverage, CoverageReason, DeepJob, DeepLimits, InspectionSession, Progress, ScanControl,
    SessionState, SessionStatus,
};
pub use topology::{
    BudgetKind, Containment, DiagnosticSeverity, EntityIdentity, PageIdentity, PhysicalEvidence,
    Relationship, RelationshipClaim, RelationshipKind, RelationshipState, StructuralDiagnostic,
    TopologyCoverage, TopologyCoverageReason, TopologyPhase, Traversal, TraversalBudget,
    TraversalKind, TraversalStop, TraversalStopReason, WorkProgress,
};

const SQLITE_HEADER_SIZE: usize = 100;
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const MINIMUM_USABLE_SIZE: u32 = 480;
const MAXIMUM_PAGE_NUMBER: u64 = 4_294_967_294;

#[derive(Debug, Error)]
pub enum InspectionError {
    #[error("the snapshot was invalidated because an accepted input changed")]
    Invalidated,
    #[error("no published revision matches this request")]
    RevisionUnavailable,
    #[error("the inspection is no longer scanning")]
    NotScanning,
    #[error("the database file could not be opened read-only: {0}")]
    Open(#[source] io::Error),
    #[error("the database header is incomplete")]
    IncompleteHeader,
    #[error("the database header does not contain the SQLite format-3 magic")]
    InvalidMagic,
    #[error("the database declares an invalid page size: {0}")]
    InvalidPageSize(u32),
    #[error("the database declares an invalid reserved-byte count: {0}")]
    InvalidReservedBytes(u8),
    #[error("the database declares an unsupported text encoding: {0}")]
    InvalidTextEncoding(u32),
    #[error("the database does not contain one complete page")]
    MissingFirstPage,
    #[error("the database ends with an incomplete page")]
    IncompletePage,
    #[error(
        "the valid database header declares {declared} pages, but the file contains {physical}"
    )]
    PageCountMismatch { declared: u32, physical: u32 },
    #[error("the database contains more pages than SQLite can address")]
    TooManyPages,
    #[error("the page inventory cannot fit in memory")]
    PageInventoryTooLarge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseGeometry {
    pub page_size: u32,
    pub usable_size: u32,
    pub reserved_bytes: u8,
    pub page_count: u32,
    pub text_encoding: TextEncoding,
    pub schema_format: u32,
    pub largest_root_page: u32,
    pub incremental_vacuum: u32,
    pub file_length: u64,
    pub declared_page_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceIdentity {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSnapshot {
    pub id: String,
    pub source: SourceIdentity,
    pub geometry: DatabaseGeometry,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageEntity {
    pub number: u32,
    pub classification: PageClassification,
    pub detail: PageDetail,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectionGraph {
    pub work_coverage: Vec<WorkProgress>,
    pub operational_budget: OperationalBudget,
    pub semantic_metadata: crate::semantic::SemanticMetadata,
    pub deep_inspections: Vec<DeepEvidence>,
    pub schema: SchemaEvidence,
    pub sidecars: Vec<SidecarEvidence>,
    pub revision: u64,
    pub coverage: Coverage,
    pub snapshot: DatabaseSnapshot,
    pub pages: Vec<PageEntity>,
    pub relationship_claims: Vec<RelationshipClaim>,
    pub relationships: Vec<Relationship>,
    pub traversals: Vec<Traversal>,
    pub diagnostics: Vec<StructuralDiagnostic>,
    pub topology_coverage: TopologyCoverage,
    pub freelist: FreelistEvidence,
    pub pointer_map: PointerMapEvidence,
}

fn read_snapshot(
    file: &File,
    snapshot_id: &str,
    source: SourceIdentity,
) -> Result<DatabaseSnapshot, InspectionError> {
    let file_length = file.metadata().map_err(InspectionError::Open)?.len();
    let mut header = [0_u8; SQLITE_HEADER_SIZE];
    read_header(file, &mut header)?;

    if &header[..SQLITE_MAGIC.len()] != SQLITE_MAGIC {
        return Err(InspectionError::InvalidMagic);
    }

    let page_size = decode_page_size(&header)?;
    let reserved_bytes = header[20];
    let usable_size = page_size
        .checked_sub(u32::from(reserved_bytes))
        .filter(|size| *size >= MINIMUM_USABLE_SIZE)
        .ok_or(InspectionError::InvalidReservedBytes(reserved_bytes))?;
    let text_encoding = decode_text_encoding(&header)?;
    let page_count = decode_page_count(&header, file_length, page_size, usable_size)?;

    let snapshot = DatabaseSnapshot {
        id: snapshot_id.to_owned(),
        source,
        geometry: DatabaseGeometry {
            page_size,
            usable_size,
            reserved_bytes,
            page_count,
            text_encoding,
            schema_format: decode_u32(&header, 44),
            largest_root_page: decode_u32(&header, 52),
            incremental_vacuum: decode_u32(&header, 64),
            file_length,
            declared_page_count: decode_u32(&header, 28),
        },
    };

    Ok(snapshot)
}

fn read_header(file: &File, header: &mut [u8; SQLITE_HEADER_SIZE]) -> Result<(), InspectionError> {
    let mut read = 0;
    while read < header.len() {
        match file.read_at(&mut header[read..], read as u64) {
            Ok(0) => return Err(InspectionError::IncompleteHeader),
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(InspectionError::Open(error)),
        }
    }
    Ok(())
}

fn decode_page_size(header: &[u8; SQLITE_HEADER_SIZE]) -> Result<u32, InspectionError> {
    let encoded = u16::from_be_bytes([header[16], header[17]]);
    let page_size = if encoded == 1 {
        65_536
    } else {
        u32::from(encoded)
    };
    if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
        return Err(InspectionError::InvalidPageSize(page_size));
    }
    Ok(page_size)
}

fn decode_text_encoding(
    header: &[u8; SQLITE_HEADER_SIZE],
) -> Result<TextEncoding, InspectionError> {
    let encoded = u32::from_be_bytes([header[56], header[57], header[58], header[59]]);
    match encoded {
        1 => Ok(TextEncoding::Utf8),
        2 => Ok(TextEncoding::Utf16Le),
        3 => Ok(TextEncoding::Utf16Be),
        value => Err(InspectionError::InvalidTextEncoding(value)),
    }
}

fn decode_page_count(
    header: &[u8; SQLITE_HEADER_SIZE],
    file_length: u64,
    page_size: u32,
    usable_size: u32,
) -> Result<u32, InspectionError> {
    let page_bytes = page_size;
    let page_size = u64::from(page_size);
    if file_length < page_size {
        return Err(InspectionError::MissingFirstPage);
    }
    let page_count = file_length.div_ceil(page_size);
    // A trailing pointer map has a geometry-defined identity even when its
    // bytes are incomplete. Other incomplete-page handling remains unchanged.
    let partial_map = !file_length.is_multiple_of(page_size)
        && decode_u32(header, 52) != 0
        && u32::try_from(page_count).ok().is_some_and(|number| {
            roles::map_number(page_bytes, usable_size, number) == Some(number)
        });
    if !file_length.is_multiple_of(page_size) && !partial_map {
        return Err(InspectionError::IncompletePage);
    }

    if page_count > MAXIMUM_PAGE_NUMBER {
        return Err(InspectionError::TooManyPages);
    }
    let physical = u32::try_from(page_count).map_err(|_| InspectionError::TooManyPages)?;
    let declared = decode_u32(header, 28);
    let change_counter = decode_u32(header, 24);
    let version_valid_for = decode_u32(header, 92);
    if declared != 0 && change_counter == version_valid_for && declared != physical && !partial_map
    {
        return Err(InspectionError::PageCountMismatch { declared, physical });
    }
    Ok(physical)
}

fn decode_u32(header: &[u8; SQLITE_HEADER_SIZE], offset: usize) -> u32 {
    u32::from_be_bytes([
        header[offset],
        header[offset + 1],
        header[offset + 2],
        header[offset + 3],
    ])
}

fn sanitized_display_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "database.sqlite".into());
    name.chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}
