use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

const SQLITE_HEADER_SIZE: usize = 100;
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const MINIMUM_USABLE_SIZE: u32 = 480;
const MAXIMUM_PAGE_NUMBER: u64 = 4_294_967_294;

#[derive(Debug, Error)]
pub enum InspectionError {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageEntity {
    pub number: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectionGraph {
    pub snapshot: DatabaseSnapshot,
    pub pages: Vec<PageEntity>,
}

#[derive(Debug)]
pub struct InspectionSession {
    graph: InspectionGraph,
    // Retaining the read-only descriptor keeps the session tied to the accepted input.
    _file: File,
}

impl InspectionSession {
    /// Opens a stable `SQLite` main file as a read-only inspection session.
    ///
    /// # Errors
    ///
    /// Returns a bounded geometry error when the file cannot establish trustworthy `SQLite` page
    /// boundaries, or an I/O error when the header cannot be read.
    pub fn open(path: &Path) -> Result<Self, InspectionError> {
        let file = File::open(path).map_err(InspectionError::Open)?;
        let file_length = file.metadata().map_err(InspectionError::Open)?.len();
        let mut header = [0_u8; SQLITE_HEADER_SIZE];
        read_header(&file, &mut header)?;

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
        let page_count = decode_page_count(file_length, page_size)?;

        let mut pages = Vec::new();
        pages
            .try_reserve_exact(page_count as usize)
            .map_err(|_| InspectionError::PageInventoryTooLarge)?;
        pages.extend((1..=page_count).map(|number| PageEntity { number }));

        let snapshot = DatabaseSnapshot {
            id: Uuid::new_v4().to_string(),
            source: SourceIdentity {
                id: Uuid::new_v4().to_string(),
                display_name: sanitized_display_name(path),
            },
            geometry: DatabaseGeometry {
                page_size,
                usable_size,
                reserved_bytes,
                page_count,
                text_encoding,
            },
        };

        Ok(Self {
            graph: InspectionGraph { snapshot, pages },
            _file: file,
        })
    }

    #[must_use]
    pub fn graph(&self) -> &InspectionGraph {
        &self.graph
    }
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

fn decode_page_count(file_length: u64, page_size: u32) -> Result<u32, InspectionError> {
    let page_size = u64::from(page_size);
    if file_length < page_size {
        return Err(InspectionError::MissingFirstPage);
    }
    if !file_length.is_multiple_of(page_size) {
        return Err(InspectionError::IncompletePage);
    }

    let page_count = file_length / page_size;
    if page_count > MAXIMUM_PAGE_NUMBER {
        return Err(InspectionError::TooManyPages);
    }
    u32::try_from(page_count).map_err(|_| InspectionError::TooManyPages)
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
