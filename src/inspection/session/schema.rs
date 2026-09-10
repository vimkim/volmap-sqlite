//! Schema declarations and page attribution are independently bounded collections.
use super::InspectionSession;
use crate::inspection::{CollectionBatch, InspectionError, PageIdentity, SchemaObjectHeader};

impl InspectionSession {
    /// Reads schema declarations attributed to a page, independently of the object-list window.
    /// # Errors
    /// Rejects changed inputs, unknown revisions or pages, invalid limits and exhaustion.
    pub fn page_schema_batch(
        &self,
        revision: u64,
        page: u32,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<crate::inspection::AttributedSchemaObject>, InspectionError> {
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
        if page == 0 || page as usize > data.pages.len() {
            return Err(InspectionError::EntityUnavailable);
        }
        data.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        let schema = data
            .pages
            .schema
            .as_ref()
            .ok_or(InspectionError::StorageUnavailable)?;
        let mut items = Vec::new();
        let mut total = 0;
        let mut retained = 0_u64;
        for position in schema.objects_for_page(page) {
            let reservation = schema
                .objects
                .estimated_size(position)
                .map_err(|_| InspectionError::StorageUnavailable)?;
            if retained.saturating_add(reservation) > 8 * 1024 * 1024
                || !crate::inspection::budget::memory_available(
                    data.status.operational_budget.max_resident_bytes,
                    reservation,
                )
            {
                return Err(InspectionError::CollectionBudget);
            }
            let object = schema
                .objects
                .get(position)
                .ok_or(InspectionError::StorageUnavailable)?;
            // A stopped update can leave an index entry beyond the retained declaration's
            // accepted boundary. Such entries never authorize attribution.
            if object.root.is_none() || object.attributed_through.is_none_or(|last| page > last) {
                continue;
            }
            if total >= offset && items.len() < limit as usize {
                items.push(crate::inspection::AttributedSchemaObject {
                    object_offset: position,
                    object: object.into(),
                });
                retained = retained.saturating_add(reservation);
            }
            total += 1;
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

    /// Reads schema declarations in stable physical-cell order, omitting attributed pages.
    /// # Errors
    /// Rejects changed inputs, unknown revisions, invalid limits and resource exhaustion.
    pub fn schema_batch(
        &self,
        revision: u64,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<SchemaObjectHeader>, InspectionError> {
        let batch = self.collection_batch(revision, offset, limit, |pages| {
            pages.schema.as_ref().map(|schema| &schema.objects)
        })?;
        Ok(CollectionBatch {
            revision: batch.revision,
            offset: batch.offset,
            total: batch.total,
            next_offset: batch.next_offset,
            items: batch.items.into_iter().map(Into::into).collect(),
        })
    }

    /// Reads attributed pages for the object at a revision's schema collection offset.
    /// The offset selects an immutable declaration; it does not replace its cell identity.
    /// # Errors
    /// Rejects changed inputs, unknown revisions or objects, invalid limits and exhaustion.
    pub fn schema_page_batch(
        &self,
        revision: u64,
        object_offset: usize,
        offset: usize,
        limit: u32,
    ) -> Result<CollectionBatch<PageIdentity>, InspectionError> {
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
        data.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        let schema = data
            .pages
            .schema
            .as_ref()
            .ok_or(InspectionError::EntityUnavailable)?;
        if object_offset >= schema.objects.len() {
            return Err(InspectionError::EntityUnavailable);
        }
        let reservation = schema
            .objects
            .estimated_size(object_offset)
            .map_err(|_| InspectionError::StorageUnavailable)?
            .saturating_add(u64::from(limit) * 256);
        if reservation > 8 * 1024 * 1024
            || !crate::inspection::budget::memory_available(
                data.status.operational_budget.max_resident_bytes,
                reservation,
            )
        {
            return Err(InspectionError::CollectionBudget);
        }
        let object = schema
            .objects
            .get(object_offset)
            .ok_or(InspectionError::StorageUnavailable)?;
        let mut total = 0;
        let mut items = Vec::new();
        for page in schema.attributed_pages(&object) {
            if total >= offset && items.len() < limit as usize {
                items.push(page);
            }
            total += 1;
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
