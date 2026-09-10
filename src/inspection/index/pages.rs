//! Page queries seek stored source and target indexes without decoding unrelated records.
use super::{Sequence, StorageError, StoredRecord};
use rusqlite::params;

impl<T: StoredRecord> Sequence<T> {
    pub(in crate::inspection) fn page_positions(
        &self,
        page: u32,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<usize>, StorageError> {
        let mut positions = Vec::new();
        if limit == 0 {
            return Ok(positions);
        }
        let mut start = 0;
        for index in 0..offset.saturating_add(limit) {
            let Some(position) = self.page_position(page, start)? else {
                break;
            };
            if index >= offset {
                positions.push(position);
            }
            start = position + 1;
        }
        Ok(positions)
    }

    pub(in crate::inspection) fn page_count(&self, page: u32) -> Result<usize, StorageError> {
        self.context.check()?;
        let memory = self
            .memory
            .values()
            .filter(|value| touches(*value, page))
            .count();
        let disk: usize = if self.spilled {
            self.context.disk()?.connection.lock().map_err(|_| StorageError::Unavailable)?
                .query_row(
                    "SELECT
                     (SELECT count(*) FROM records WHERE collection=?1 AND source_page=?2) +
                     (SELECT count(*) FROM records WHERE collection=?1 AND target_page=?2 AND source_page IS NOT ?2)",
                    params![self.collection, page], |row| row.get(0),
                )?
        } else {
            0
        };
        let local = memory + disk;
        self.segments.iter().try_fold(
            local,
            |total, segment| Ok(total + segment.page_count(page)?),
        )
    }

    /// Returns a position in sequence order, at or after `start`, touching the page.
    pub(in crate::inspection) fn page_position(
        &self,
        page: u32,
        start: usize,
    ) -> Result<Option<usize>, StorageError> {
        self.context.check()?;
        if start < self.length {
            let local = self.local_page_position(page, start)?;
            if local.is_some() {
                return Ok(local);
            }
        }
        let mut base = self.length;
        for segment in &self.segments {
            if start < base + segment.len()
                && let Some(position) = segment.page_position(page, start.saturating_sub(base))?
            {
                return Ok(Some(base + position));
            }
            base += segment.len();
        }
        Ok(None)
    }

    fn local_page_position(&self, page: u32, start: usize) -> Result<Option<usize>, StorageError> {
        let memory = self
            .memory
            .range(start..)
            .find(|(_, value)| touches(*value, page))
            .map(|(position, _)| *position);
        if !self.spilled {
            return Ok(memory);
        }
        let disk = self.context.disk()?;
        let connection = disk
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?;
        let source: Option<usize> = connection.query_row(
            "SELECT min(position) FROM records WHERE collection=?1 AND source_page=?2 AND position>=?3",
            params![self.collection, page, start], |row| row.get(0),
        )?;
        let target: Option<usize> = connection.query_row(
            "SELECT min(position) FROM records WHERE collection=?1 AND target_page=?2 AND position>=?3",
            params![self.collection, page, start], |row| row.get(0),
        )?;
        Ok([memory, source, target].into_iter().flatten().min())
    }
}

fn touches<T: StoredRecord>(value: &T, page: u32) -> bool {
    let (source, target) = value.page_tags();
    source == Some(page) || target == Some(page)
}
