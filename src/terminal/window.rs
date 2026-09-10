//! Terminal projections retain only the current navigation windows.
use super::{Focus, TerminalFlow};
use crate::inspection::{CollectionBatch, InspectionError, InspectionGraph};
use std::sync::Arc;

pub(super) const SIZE: u32 = 32;

struct Projection {
    graph: InspectionGraph,
    ranges: Vec<Range>,
    schema_positions: Vec<usize>,
    traversal_headers: Vec<crate::inspection::TraversalHeader>,
}

pub(super) struct Range {
    name: &'static str,
    offset: usize,
    count: usize,
    total: usize,
    next: Option<usize>,
}
impl Range {
    pub(super) fn label(&self) -> String {
        let window = if self.count == 0 {
            "0".into()
        } else {
            format!("{}-{}", self.offset + 1, self.offset + self.count)
        };
        format!(
            "{}: {window} of {}; </> changes the displayed window",
            self.name, self.total
        )
    }
    fn slice(name: &'static str, offset: usize, total: usize) -> Self {
        let end = offset.saturating_add(SIZE as usize).min(total);
        Self {
            name,
            offset,
            total,
            count: end.saturating_sub(offset),
            next: (end < total).then_some(end),
        }
    }
}

fn retain<T>(ranges: &mut Vec<Range>, name: &'static str, batch: CollectionBatch<T>) -> Vec<T> {
    ranges.push(Range {
        name,
        offset: batch.offset,
        count: batch.items.len(),
        total: batch.total,
        next: batch.next_offset,
    });
    batch.items
}

impl TerminalFlow {
    pub(super) fn next_window(&self) -> Option<usize> {
        self.ranges.iter().filter_map(|range| range.next).min()
    }
    pub(super) fn set_window(&mut self, offset: usize) {
        self.withhold_values();
        self.location_mut().offset = offset;
        self.location_mut().selected = 0;
        self.evidence_offset = 0;
        self.graph = None;
    }
    pub(super) fn shift_window(&mut self, forward: bool) {
        if forward {
            if let Some(next) = self.next_window() {
                self.set_window(next);
            }
        } else {
            self.set_window(self.location().offset.saturating_sub(SIZE as usize));
        }
    }
    pub(super) fn load_window(&mut self) {
        match self.read_window() {
            Ok(projection) => {
                self.graph = Some(Arc::new(projection.graph));
                self.ranges = projection.ranges;
                self.schema_positions = projection.schema_positions;
                self.traversal_headers = projection.traversal_headers;
            }
            Err(error) => {
                self.graph = None;
                self.ranges.clear();
                self.schema_positions.clear();
                self.traversal_headers.clear();
                self.message = format!("Evidence window unavailable: {error}");
            }
        }
    }
    fn read_window(&self) -> Result<Projection, InspectionError> {
        let mut graph = self
            .session
            .revision_metadata(self.revision)?
            .into_projection();
        let mut ranges = Vec::new();
        let mut schema_positions = Vec::new();
        let mut traversal_headers = Vec::new();
        let offset = self.location().offset;
        match self.focus() {
            Focus::Traversal(position) => {
                self.read_traversal_window(&mut graph, &mut ranges, *position, offset)?;
            }
            Focus::Schema => {
                let batch = self.session.schema_batch(self.revision, offset, SIZE)?;
                graph.schema.objects = retain(&mut ranges, "Schema objects", batch)
                    .into_iter()
                    .map(|header| header.with_pages(Vec::new()))
                    .collect();
            }
            Focus::Object(identity, position) => {
                self.read_object_window(&mut graph, &mut ranges, identity, *position, offset)?;
            }
            Focus::Btrees => {
                let first = u32::try_from(offset)
                    .ok()
                    .and_then(|offset| offset.checked_add(1))
                    .ok_or(InspectionError::InvalidPageRange)?;
                let batch = self.session.page_batch(self.revision, first, SIZE)?;
                ranges.push(Range {
                    name: "Physical pages (B-tree filter)",
                    offset,
                    count: batch.pages.len(),
                    total: batch.total as usize,
                    next: batch.next_page.map(|page| page as usize - 1),
                });
                graph.pages = batch.pages;
            }
            Focus::Freelist => {
                graph.freelist.trunks = retain(
                    &mut ranges,
                    "Freelist trunks",
                    self.session
                        .freelist_trunk_batch(self.revision, offset, SIZE)?,
                );
            }
            Focus::PointerMaps => {
                graph.pointer_map.pages = retain(
                    &mut ranges,
                    "Pointer maps",
                    self.session
                        .pointer_map_batch(self.revision, offset, SIZE)?,
                );
                graph.pointer_map.locations = graph
                    .pointer_map
                    .pages
                    .iter()
                    .map(|page| page.page.clone())
                    .collect();
            }
            Focus::Diagnostics => {
                graph.diagnostics = retain(
                    &mut ranges,
                    "Diagnostics",
                    self.session.diagnostic_batch(self.revision, offset, SIZE)?,
                );
            }
            Focus::Page(page)
            | Focus::Cell(crate::inspection::CellIdentity {
                page_number: page, ..
            }) => {
                schema_positions = self.read_page_window(&mut graph, &mut ranges, *page, offset)?;
                traversal_headers = retain(
                    &mut ranges,
                    "Page traversals",
                    self.session.traversal_header_batch(
                        self.revision,
                        Some(*page),
                        offset,
                        SIZE,
                    )?,
                );
            }
            Focus::Database | Focus::Help | Focus::WindowOffset(_) => (),
        }
        Ok(Projection {
            graph,
            ranges,
            schema_positions,
            traversal_headers,
        })
    }

    fn read_object_window(
        &self,
        graph: &mut InspectionGraph,
        ranges: &mut Vec<Range>,
        identity: &crate::inspection::CellIdentity,
        position: usize,
        offset: usize,
    ) -> Result<(), InspectionError> {
        let header = self
            .session
            .schema_batch(self.revision, position, 1)?
            .items
            .pop()
            .filter(|object| object.identity == *identity)
            .ok_or(InspectionError::EntityUnavailable)?;
        let pages = retain(
            ranges,
            "Attributed pages",
            self.session
                .schema_page_batch(self.revision, position, offset, SIZE)?,
        );
        graph.schema.objects.push(header.with_pages(pages));
        Ok(())
    }

    fn read_traversal_window(
        &self,
        graph: &mut InspectionGraph,
        ranges: &mut Vec<Range>,
        position: usize,
        offset: usize,
    ) -> Result<(), InspectionError> {
        let header = self
            .session
            .traversal_header_batch(self.revision, None, position, 1)?
            .items
            .pop()
            .ok_or(InspectionError::EntityUnavailable)?;
        let steps = retain(
            ranges,
            "Traversal prefix pages",
            self.session
                .traversal_prefix_batch(self.revision, position, offset, SIZE)?,
        );
        graph.traversals.push(crate::inspection::Traversal {
            kind: header.kind,
            origin: header.origin,
            stop: header.stop,
            validated_prefix: steps,
        });
        Ok(())
    }

    fn read_page_window(
        &self,
        graph: &mut InspectionGraph,
        ranges: &mut Vec<Range>,
        page: u32,
        offset: usize,
    ) -> Result<Vec<usize>, InspectionError> {
        graph.pages = self.session.page_batch(self.revision, page, 1)?.pages;
        let observed = graph
            .pages
            .first()
            .ok_or(InspectionError::EntityUnavailable)?;
        ranges.push(Range::slice(
            "Byte regions",
            offset,
            observed.detail.regions.len(),
        ));
        ranges.push(Range::slice(
            "Role claims",
            offset,
            observed.classification.claims.len(),
        ));
        if matches!(self.focus(), Focus::Page(_)) {
            ranges.push(Range::slice("Cells", offset, observed.detail.cells.len()));
        }
        graph.relationship_claims = retain(
            ranges,
            "Page claims",
            self.session
                .page_claim_batch(self.revision, page, offset, SIZE)?,
        );
        graph.relationships = retain(
            ranges,
            "Page relationships",
            self.session
                .page_relationship_batch(self.revision, page, offset, SIZE)?,
        );
        if let Some(map) = self.session.page_pointer_map(self.revision, page)? {
            ranges.push(Range::slice(
                "Pointer-map entries",
                offset,
                map.entries.len(),
            ));
            graph.pointer_map.pages.push(map);
        }
        if let Some(trunk) = self.session.page_freelist_trunk(self.revision, page)? {
            graph.freelist.trunks.push(trunk);
        }
        let associations = retain(
            ranges,
            "Page schema objects",
            self.session
                .page_schema_batch(self.revision, page, offset, SIZE)?,
        );
        let mut positions = Vec::with_capacity(associations.len());
        for association in associations {
            positions.push(association.object_offset);
            graph.schema.objects.push(
                association
                    .object
                    .with_pages(vec![crate::inspection::PageIdentity { page_number: page }]),
            );
        }
        Ok(positions)
    }
}
