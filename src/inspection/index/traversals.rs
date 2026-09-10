//! Private traversal framing permits bounded reads of arbitrarily long prefixes.
use super::{Sequence, StorageError};
use crate::inspection::{PageIdentity, Traversal, TraversalHeader};
use rusqlite::params;

pub(super) fn encode(value: &Traversal) -> Result<Vec<u8>, serde_json::Error> {
    let header = serde_json::to_vec(&TraversalHeader::from_traversal(value, 0))?;
    let length = u32::try_from(header.len()).map_err(invalid)?;
    let mut bytes = Vec::with_capacity(4 + header.len() + value.validated_prefix.len() * 4);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&header);
    for page in &value.validated_prefix {
        bytes.extend_from_slice(&page.page_number.to_le_bytes());
    }
    Ok(bytes)
}

fn invalid(error: impl std::fmt::Display) -> serde_json::Error {
    <serde_json::Error as serde::de::Error>::custom(error)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Traversal, serde_json::Error> {
    let size = bytes
        .get(..4)
        .ok_or_else(|| invalid("missing traversal frame"))?;
    let length = u32::from_le_bytes(size.try_into().map_err(invalid)?) as usize;
    let end = 4_usize
        .checked_add(length)
        .ok_or_else(|| invalid("invalid header length"))?;
    let header: TraversalHeader =
        serde_json::from_slice(bytes.get(4..end).ok_or_else(|| invalid("missing header"))?)?;
    let prefix = bytes.get(end..).ok_or_else(|| invalid("missing prefix"))?;
    if prefix.len() % 4 != 0 || prefix.len() / 4 != header.prefix_count {
        return Err(invalid("invalid prefix length"));
    }
    Ok(Traversal {
        kind: header.kind,
        origin: header.origin,
        stop: header.stop,
        validated_prefix: prefix
            .chunks_exact(4)
            .map(|bytes| PageIdentity {
                page_number: u32::from_le_bytes(bytes.try_into().expect("four-byte step")),
            })
            .collect(),
    })
}

impl Sequence<Traversal> {
    pub(crate) fn traversal_window(
        &self,
        position: usize,
        offset: usize,
        limit: usize,
    ) -> Result<(TraversalHeader, Vec<PageIdentity>), StorageError> {
        self.context.check()?;
        if position >= self.length {
            let mut remaining = position - self.length;
            for segment in &self.segments {
                if remaining < segment.len() {
                    let (mut header, steps) = segment.traversal_window(remaining, offset, limit)?;
                    header.traversal_offset = position;
                    return Ok((header, steps));
                }
                remaining -= segment.len();
            }
            return Err(StorageError::Unavailable);
        }
        if let Some(value) = self.memory.get(&position) {
            return Ok((
                TraversalHeader::from_traversal(value, position),
                value
                    .validated_prefix
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .cloned()
                    .collect(),
            ));
        }
        let result = self.read_traversal_blob(position, offset, limit);
        if let Err(error) = result {
            self.context.record_error(error);
        }
        result
    }

    fn read_traversal_blob(
        &self,
        position: usize,
        offset: usize,
        limit: usize,
    ) -> Result<(TraversalHeader, Vec<PageIdentity>), StorageError> {
        let disk = self.context.disk()?;
        let connection = disk
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?;
        let row: i64 = connection.query_row(
            "SELECT rowid FROM records WHERE collection=?1 AND position=?2",
            params![self.collection, position],
            |row| row.get(0),
        )?;
        let blob = connection.blob_open("main", "records", "evidence", row, true)?;
        let mut size = [0; 4];
        blob.read_at_exact(&mut size, 0)?;
        let length = u32::from_le_bytes(size) as usize;
        if length > 65536 {
            return Err(StorageError::Unavailable);
        }
        let mut bytes = vec![0; length];
        blob.read_at_exact(&mut bytes, 4)?;
        let mut header: TraversalHeader =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::Unavailable)?;
        let prefix_start = length + 4;
        let expected = header
            .prefix_count
            .checked_mul(4)
            .and_then(|n| n.checked_add(prefix_start));
        if expected != Some(blob.len()) {
            return Err(StorageError::Unavailable);
        }
        header.traversal_offset = position;
        let count = header.prefix_count.saturating_sub(offset).min(limit);
        let mut bytes = vec![0; count * 4];
        if count > 0 {
            blob.read_at_exact(&mut bytes, prefix_start + offset * 4)?;
        }
        let steps = bytes
            .chunks_exact(4)
            .map(|bytes| PageIdentity {
                page_number: u32::from_le_bytes(bytes.try_into().expect("four-byte step")),
            })
            .collect();
        Ok((header, steps))
    }
}
