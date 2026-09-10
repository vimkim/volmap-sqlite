//! Bounded, revision-scoped collection windows shared by the adapters.
use super::InspectionSession;
use crate::inspection::index::{Sequence, StoredRecord};
use crate::inspection::storage::PageStore;
use crate::inspection::{CollectionBatch, InspectionError};

impl InspectionSession {
    /// Reads a bounded window of retained claims in storage order.
    /// # Errors
    /// Rejects changed inputs, unknown revisions, invalid limits and resource exhaustion.
    pub fn claim_batch(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<crate::inspection::RelationshipClaim>, InspectionError> {
        self.collection_batch(revision, offset, limit, |pages| Some(&pages.claims))
    }

    /// Reads a bounded window of retained relationships in storage order.
    /// # Errors
    /// Rejects changed inputs, unknown revisions, invalid limits and resource exhaustion.
    pub fn relationship_batch(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<crate::inspection::Relationship>, InspectionError> {
        self.collection_batch(revision, offset, limit, |pages| Some(&pages.relationships))
    }

    /// Reads a bounded window of retained traversals in storage order.
    /// # Errors
    /// Rejects changed inputs, unknown revisions, invalid limits and resource exhaustion.
    pub fn traversal_batch(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<crate::inspection::Traversal>, InspectionError> {
        self.collection_batch(revision, offset, limit, |pages| Some(&pages.traversals))
    }

    /// Reads a bounded window of retained diagnostics in storage order.
    /// # Errors
    /// Rejects changed inputs, unknown revisions, invalid limits and resource exhaustion.
    pub fn diagnostic_batch(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<crate::inspection::StructuralDiagnostic>, InspectionError> {
        self.collection_batch(revision, offset, limit, |pages| Some(&pages.diagnostics))
    }

    /// Reads a bounded window of retained freelist trunks in storage order.
    /// # Errors
    /// Rejects changed inputs, unknown revisions, invalid limits and resource exhaustion.
    pub fn freelist_trunk_batch(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<crate::inspection::FreelistTrunk>, InspectionError> {
        self.collection_batch(revision, offset, limit, |pages| {
            Some(&pages.freelist_trunks)
        })
    }

    /// Reads a bounded window of retained pointer maps in storage order.
    /// # Errors
    /// Rejects changed inputs, unknown revisions, invalid limits and resource exhaustion.
    pub fn pointer_map_batch(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<crate::inspection::PointerMapPage>, InspectionError> {
        self.collection_batch(revision, offset, limit, |pages| Some(&pages.pointer_maps))
    }

    pub(super) fn collection_batch<T: StoredRecord>(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
        select: impl FnOnce(&PageStore) -> Option<&Sequence<T>>,
    ) -> Result<CollectionBatch<T>, InspectionError> {
        self.collection_query(revision, None, offset, limit, select)
    }

    pub(super) fn collection_query<T: StoredRecord>(
        &self,
        revision: u64,
        page: Option<u32>,
        offset: usize,
        limit: u32,
        select: impl FnOnce(&PageStore) -> Option<&Sequence<T>>,
    ) -> Result<CollectionBatch<T>, InspectionError> {
        if limit == 0 || limit > 256 {
            return Err(InspectionError::InvalidCollectionRange);
        }
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
        let collection = select(&data.pages);
        let total = match (page, collection) {
            (Some(page), Some(collection)) => collection
                .page_count(page)
                .map_err(|_| InspectionError::StorageUnavailable)?,
            (_, collection) => collection.map_or(0, Sequence::len),
        };
        let end = offset.saturating_add(limit as usize).min(total);
        let positions = match (page, collection) {
            (Some(page), Some(collection)) => collection
                .page_positions(page, offset, end.saturating_sub(offset))
                .map_err(|_| InspectionError::StorageUnavailable)?,
            _ => (offset..end).collect(),
        };
        let mut items = Vec::new();
        let mut retained = 0_u64;
        for position in positions {
            let collection = collection.ok_or(InspectionError::StorageUnavailable)?;
            let reservation = collection
                .estimated_size(position)
                .map_err(|_| InspectionError::StorageUnavailable)?;
            // One dense page-backed record may need more decoded headroom than
            // the ordinary window. Estimates include 4x serialized bytes.
            let window_budget = if items.is_empty() {
                32 * 1024 * 1024
            } else {
                8 * 1024 * 1024
            };
            if retained.saturating_add(reservation) > window_budget
                || !crate::inspection::budget::memory_available(
                    data.status.operational_budget.max_resident_bytes,
                    reservation,
                )
            {
                if items.is_empty() {
                    return Err(InspectionError::CollectionBudget);
                }
                break;
            }
            items.push(
                collection
                    .get(position)
                    .ok_or(InspectionError::StorageUnavailable)?,
            );
            retained = retained.saturating_add(reservation);
        }
        data.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        let next = offset.saturating_add(items.len());
        Ok(CollectionBatch {
            revision,
            offset,
            total,
            next_offset: (next < total && next > offset).then_some(next),
            items,
        })
    }
}
