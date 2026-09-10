//! Revision headers omit collections; readers request their bounded windows separately.
use super::{
    CellIdentity, DeepEvidence, FreelistCoverage, FreelistField, OperationalBudget, PageIdentity,
    PointerMapLayout, RevisionSummary, SchemaState, SidecarEvidence, WorkProgress,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionMetadata {
    pub summary: RevisionSummary,
    pub work_coverage: Vec<WorkProgress>,
    pub operational_budget: OperationalBudget,
    pub semantic_metadata: crate::semantic::SemanticMetadata,
    pub deep_inspections: Vec<DeepEvidence>,
    pub sidecars: Vec<SidecarEvidence>,
    pub schema: SchemaHeader,
    pub freelist: FreelistHeader,
    pub pointer_map: PointerMapHeader,
}

impl RevisionMetadata {
    /// Header-only working projection for adapters that fill explicit collection windows.
    pub(crate) fn into_projection(self) -> super::InspectionGraph {
        super::InspectionGraph {
            snapshot: self.summary.snapshot,
            revision: self.summary.revision,
            coverage: self.summary.coverage,
            topology_coverage: self.summary.topology_coverage,
            work_coverage: self.work_coverage,
            operational_budget: self.operational_budget,
            semantic_metadata: self.semantic_metadata,
            deep_inspections: self.deep_inspections,
            sidecars: self.sidecars,
            pages: Vec::new(),
            relationship_claims: Vec::new(),
            relationships: Vec::new(),
            traversals: Vec::new(),
            diagnostics: Vec::new(),
            schema: super::SchemaEvidence {
                state: self.schema.state,
                diagnostics: self.schema.diagnostics,
                max_decoded_bytes: self.schema.max_decoded_bytes,
                decoded_bytes: self.schema.decoded_bytes,
                stopping_cell: self.schema.stopping_cell,
                objects: Vec::new(),
            },
            freelist: super::FreelistEvidence {
                first_trunk: self.freelist.first_trunk,
                declared_count: self.freelist.declared_count,
                coverage: self.freelist.coverage,
                trunks: Vec::new(),
            },
            pointer_map: super::PointerMapEvidence {
                applicable: self.pointer_map.applicable,
                diagnostics: self.pointer_map.diagnostics,
                layout: self.pointer_map.layout,
                lock_byte_page: self.pointer_map.lock_byte_page,
                complete: self.pointer_map.complete,
                largest_root: self.pointer_map.largest_root,
                incremental_vacuum: self.pointer_map.incremental_vacuum,
                pages: Vec::new(),
                locations: Vec::new(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaHeader {
    pub state: SchemaState,
    pub diagnostics: Vec<&'static str>,
    pub max_decoded_bytes: String,
    pub decoded_bytes: String,
    pub stopping_cell: Option<CellIdentity>,
}
impl From<&super::SchemaEvidence> for SchemaHeader {
    fn from(evidence: &super::SchemaEvidence) -> Self {
        Self {
            state: evidence.state,
            diagnostics: evidence.diagnostics.clone(),
            max_decoded_bytes: evidence.max_decoded_bytes.clone(),
            decoded_bytes: evidence.decoded_bytes.clone(),
            stopping_cell: evidence.stopping_cell.clone(),
        }
    }
}

/// A schema declaration. Attributed pages have their own bounded query.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObjectHeader {
    pub identity: CellIdentity,
    pub evidence: Vec<super::PhysicalEvidence>,
    pub object_type: Option<super::SchemaObjectType>,
    pub name: Option<String>,
    pub table_name: Option<String>,
    pub root_page: Option<String>,
    pub declaration: Option<String>,
    pub root: Option<PageIdentity>,
    pub state: SchemaState,
    pub diagnostics: Vec<std::borrow::Cow<'static, str>>,
}

/// A schema declaration attributed to a page, with its stable collection position.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributedSchemaObject {
    pub object_offset: usize,
    pub object: SchemaObjectHeader,
}
impl From<super::SchemaObject> for SchemaObjectHeader {
    fn from(object: super::SchemaObject) -> Self {
        Self {
            identity: object.identity,
            evidence: object.evidence,
            object_type: object.object_type,
            name: object.name,
            table_name: object.table_name,
            root_page: object.root_page,
            declaration: object.declaration,
            root: object.root,
            state: object.state,
            diagnostics: object.diagnostics,
        }
    }
}

impl SchemaObjectHeader {
    pub(crate) fn with_pages(self, pages: Vec<PageIdentity>) -> super::SchemaObject {
        super::SchemaObject {
            identity: self.identity,
            evidence: self.evidence,
            object_type: self.object_type,
            name: self.name,
            table_name: self.table_name,
            root_page: self.root_page,
            declaration: self.declaration,
            root: self.root,
            state: self.state,
            diagnostics: self.diagnostics,
            pages,
            attributed_through: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreelistHeader {
    pub first_trunk: Option<FreelistField>,
    pub declared_count: Option<FreelistField>,
    pub coverage: FreelistCoverage,
}
impl From<&super::FreelistEvidence> for FreelistHeader {
    fn from(evidence: &super::FreelistEvidence) -> Self {
        Self {
            first_trunk: evidence.first_trunk.clone(),
            declared_count: evidence.declared_count.clone(),
            coverage: evidence.coverage.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerMapHeader {
    pub applicable: bool,
    pub diagnostics: Vec<&'static str>,
    pub layout: Option<PointerMapLayout>,
    pub lock_byte_page: Option<PageIdentity>,
    pub complete: bool,
    pub largest_root: FreelistField,
    pub incremental_vacuum: FreelistField,
}
impl From<&super::PointerMapEvidence> for PointerMapHeader {
    fn from(evidence: &super::PointerMapEvidence) -> Self {
        Self {
            applicable: evidence.applicable,
            diagnostics: evidence.diagnostics.clone(),
            layout: evidence.layout.clone(),
            lock_byte_page: evidence.lock_byte_page.clone(),
            complete: evidence.complete,
            largest_root: evidence.largest_root.clone(),
            incremental_vacuum: evidence.incremental_vacuum.clone(),
        }
    }
}
/// Traversal identity, termination and prefix length without loading prefix steps.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraversalHeader {
    pub traversal_offset: usize,
    pub kind: super::TraversalKind,
    pub origin: super::EntityIdentity,
    pub prefix_count: usize,
    pub stop: Option<super::TraversalStop>,
}

impl TraversalHeader {
    pub(crate) fn from_traversal(traversal: &super::Traversal, position: usize) -> Self {
        Self {
            traversal_offset: position,
            kind: traversal.kind,
            origin: traversal.origin.clone(),
            prefix_count: traversal.validated_prefix.len(),
            stop: traversal.stop.clone(),
        }
    }
}
