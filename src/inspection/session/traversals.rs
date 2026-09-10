//! Traversal headers and prefix steps have independent bounded windows.
use super::InspectionSession;
use crate::inspection::{CollectionBatch, InspectionError, PageIdentity, TraversalHeader};

impl InspectionSession {
    /// Reads traversal headers, optionally restricted to an originating page.
    /// # Errors
    /// Rejects invalid scopes, limits, changed inputs and resource exhaustion.
    pub fn traversal_header_batch(
        &self,
        revision: u64,
        page: Option<u32>,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<TraversalHeader>, InspectionError> {
        validate_limit(limit)?;
        self.verify(|| {})?;
        let mut data = self.lock();
        if !data.validate() {
            return Err(InspectionError::Invalidated);
        }
        if !data.revisions.contains_key(&revision) {
            return Err(InspectionError::RevisionUnavailable);
        }
        if page.is_some_and(|page| page == 0 || page as usize > data.pages.len()) {
            return Err(InspectionError::EntityUnavailable);
        }
        admit(data.status.operational_budget.max_resident_bytes, limit)?;
        let records = &data.pages.traversals;
        let total = match page {
            Some(page) => records
                .page_count(page)
                .map_err(|_| InspectionError::StorageUnavailable)?,
            None => records.len(),
        };
        let positions = match page {
            Some(page) => records
                .page_positions(page, offset, limit as usize)
                .map_err(|_| InspectionError::StorageUnavailable)?,
            None => (offset..offset.saturating_add(limit as usize).min(total)).collect(),
        };
        let mut items = Vec::new();
        for position in positions {
            items.push(
                records
                    .traversal_window(position, 0, 0)
                    .map_err(|_| InspectionError::StorageUnavailable)?
                    .0,
            );
        }
        data.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        Ok(batch(revision, offset, total, items))
    }

    /// Reads at most 256 prefix steps without decoding the remaining traversal.
    /// # Errors
    /// Rejects unknown revisions/traversals, changed inputs and resource exhaustion.
    pub fn traversal_prefix_batch(
        &self,
        revision: u64,
        traversal_offset: usize,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<PageIdentity>, InspectionError> {
        validate_limit(limit)?;
        self.verify(|| {})?;
        let mut data = self.lock();
        if !data.validate() {
            return Err(InspectionError::Invalidated);
        }
        if !data.revisions.contains_key(&revision) {
            return Err(InspectionError::RevisionUnavailable);
        }
        if traversal_offset >= data.pages.traversals.len() {
            return Err(InspectionError::EntityUnavailable);
        }
        admit(data.status.operational_budget.max_resident_bytes, 1)?;
        let (header, items) = data
            .pages
            .traversals
            .traversal_window(traversal_offset, offset, limit as usize)
            .map_err(|_| InspectionError::StorageUnavailable)?;
        data.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        Ok(batch(revision, offset, header.prefix_count, items))
    }
}

fn validate_limit(limit: u32) -> Result<(), InspectionError> {
    if limit == 0 || limit > 256 {
        Err(InspectionError::InvalidCollectionRange)
    } else {
        Ok(())
    }
}

fn admit(ceiling: u64, headers: u32) -> Result<(), InspectionError> {
    if crate::inspection::budget::memory_available(ceiling, u64::from(headers) * 262_144 + 65_536) {
        Ok(())
    } else {
        Err(InspectionError::CollectionBudget)
    }
}

fn batch<T>(revision: u64, offset: usize, total: usize, items: Vec<T>) -> CollectionBatch<T> {
    let next = offset.saturating_add(items.len());
    CollectionBatch {
        revision,
        offset,
        total,
        next_offset: (next < total && next > offset).then_some(next),
        items,
    }
}
