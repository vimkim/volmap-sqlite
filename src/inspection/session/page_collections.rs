//! Local navigation reads only records whose stored page keys match the selection.
use super::InspectionSession;
use crate::inspection::{
    CollectionBatch, InspectionError, Relationship, RelationshipClaim, Traversal,
};

impl InspectionSession {
    /// Reads a retained pointer-map record for the selected page.
    /// # Errors
    /// Rejects invalid snapshots, revisions, pages and resource exhaustion.
    pub fn page_pointer_map(
        &self,
        revision: u64,
        page: u32,
    ) -> Result<Option<crate::inspection::PointerMapPage>, InspectionError> {
        Ok(self
            .collection_query(revision, Some(page), 0, 1, |pages| {
                Some(&pages.pointer_maps)
            })?
            .items
            .pop())
    }

    /// Reads a retained freelist-trunk record for the selected page.
    /// # Errors
    /// Rejects invalid snapshots, revisions, pages and resource exhaustion.
    pub fn page_freelist_trunk(
        &self,
        revision: u64,
        page: u32,
    ) -> Result<Option<crate::inspection::FreelistTrunk>, InspectionError> {
        Ok(self
            .collection_query(revision, Some(page), 0, 1, |pages| {
                Some(&pages.freelist_trunks)
            })?
            .items
            .pop())
    }

    /// Reads incoming and outgoing claims touching a retained page, without duplicates.
    /// # Errors
    /// Rejects invalid snapshots, revisions, pages, limits and resource exhaustion.
    pub fn page_claim_batch(
        &self,
        revision: u64,
        page: u32,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<RelationshipClaim>, InspectionError> {
        self.collection_query(revision, Some(page), offset, limit, |pages| {
            Some(&pages.claims)
        })
    }

    /// Reads incoming and outgoing normalized relationships touching a retained page.
    /// # Errors
    /// Rejects invalid snapshots, revisions, pages, limits and resource exhaustion.
    pub fn page_relationship_batch(
        &self,
        revision: u64,
        page: u32,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<Relationship>, InspectionError> {
        self.collection_query(revision, Some(page), offset, limit, |pages| {
            Some(&pages.relationships)
        })
    }

    /// Reads traversals originating at a retained page or one of its cells.
    /// # Errors
    /// Rejects invalid snapshots, revisions, pages, limits and resource exhaustion.
    pub fn page_traversal_batch(
        &self,
        revision: u64,
        page: u32,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<Traversal>, InspectionError> {
        self.collection_query(revision, Some(page), offset, limit, |pages| {
            Some(&pages.traversals)
        })
    }
}
