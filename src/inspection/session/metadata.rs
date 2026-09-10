use super::InspectionSession;
use crate::inspection::storage::PageStore;
use crate::inspection::{InspectionError, InspectionGraph, RevisionMetadata, RevisionSummary};

pub(super) fn summary(graph: &InspectionGraph, pages: &PageStore) -> RevisionSummary {
    RevisionSummary {
        snapshot: graph.snapshot.clone(),
        revision: graph.revision,
        coverage: graph.coverage.clone(),
        topology_coverage: graph.topology_coverage,
        page_count: graph.coverage.evaluated,
        relationship_count: pages.relationships.len(),
        claim_count: pages.claims.len(),
        traversal_count: pages.traversals.len(),
        diagnostic_count: pages.diagnostics.len(),
        schema_state: graph.schema.state,
        schema_object_count: pages
            .schema
            .as_ref()
            .map_or(0, |schema| schema.objects.len()),
        freelist_trunk_count: pages.freelist_trunks.len(),
        pointer_map_page_count: pages.pointer_maps.len(),
    }
}

impl InspectionSession {
    /// Reads revision headers and exact collection counts without loading the collections.
    /// # Errors
    /// Rejects changed inputs, unknown revisions and insufficient response memory.
    pub fn revision_metadata(&self, revision: u64) -> Result<RevisionMetadata, InspectionError> {
        self.verify(|| {})?;
        let mut data = self.lock();
        if !data.validate() {
            return Err(InspectionError::Invalidated);
        }
        let graph = data
            .revisions
            .get(&revision)
            .ok_or(InspectionError::RevisionUnavailable)?;
        let reservation = crate::inspection::budget::serialized_reservation(graph.as_ref())
            .ok_or(InspectionError::StorageUnavailable)?;
        if !crate::inspection::budget::memory_available(
            data.status.operational_budget.max_resident_bytes,
            reservation,
        ) {
            return Err(InspectionError::CollectionBudget);
        }
        data.pages
            .check()
            .map_err(|_| InspectionError::StorageUnavailable)?;
        Ok(RevisionMetadata {
            summary: summary(graph, &data.pages),
            work_coverage: graph.work_coverage.clone(),
            operational_budget: graph.operational_budget,
            semantic_metadata: graph.semantic_metadata.clone(),
            deep_inspections: graph.deep_inspections.clone(),
            sidecars: graph.sidecars.clone(),
            schema: (&graph.schema).into(),
            freelist: (&graph.freelist).into(),
            pointer_map: (&graph.pointer_map).into(),
        })
    }
}
