use super::{Focus, entity_page};
use crate::inspection::{InspectionGraph, PhysicalEvidence};

const HELP: &[&str] = &[
    "Keyboard help",
    "Up/k Down/j: move; Enter: enter target; Esc/Backspace: back",
    "PgUp/PgDn: vertical evidence pages; h/l: horizontal evidence scroll; 0: reset scroll",
    "!: diagnostics; ?: help; q: quit",
    "g: go to page[:cell]; s: stop fast scan with partial coverage",
    "d: deep-inspect selected cell; c: cancel work; [: older revision; ]: newer revision",
    "Only explicit deep inspection reveals typed values. Raw payload bytes are never shown.",
];

pub(super) fn lines(focus: &Focus, graph: Option<&InspectionGraph>) -> Vec<String> {
    if *focus == Focus::Help {
        return HELP.iter().map(|line| (*line).to_owned()).collect();
    }
    let Some(graph) = graph else {
        return vec!["Waiting for a published inspection; progress is not a graph.".into()];
    };
    let mut lines = Vec::new();
    match focus {
        Focus::Database => {
            lines.push(format!("Snapshot: {}", graph.snapshot.id));
            let geometry = &graph.snapshot.geometry;
            lines.push(format!(
                "{} pages | page size {} | usable {} | reserved {}",
                geometry.page_count,
                geometry.page_size,
                geometry.usable_size,
                geometry.reserved_bytes
            ));
            lines.push(format!(
                "Topology coverage: {:?}",
                graph.topology_coverage.reason
            ));
            for sidecar in &graph.sidecars {
                lines.push(format!(
                    "{}: {} — {}",
                    sidecar.kind, sidecar.state, sidecar.consequence
                ));
            }
        }
        Focus::Schema => lines.push(format!(
            "Direct schema coverage: {:?}; {} objects",
            graph.schema.state,
            graph.schema.objects.len()
        )),
        Focus::Object(identity) => return object_lines(graph, identity),
        Focus::Page(number)
        | Focus::Cell(crate::inspection::CellIdentity {
            page_number: number,
            ..
        }) => return page_lines(graph, *number, focus),
        Focus::Btrees => lines.push(
            "B-tree storage: directly observed pages and their reconciled role claims.".into(),
        ),
        Focus::Freelist => {
            lines.push(format!(
                "Freelist coverage: {:?}; {} evaluated pages",
                graph.freelist.coverage.reason, graph.freelist.coverage.evaluated_pages
            ));
            lines.push(format!(
                "Declared free pages: {:?}",
                graph
                    .freelist
                    .declared_count
                    .as_ref()
                    .map(|field| field.value)
            ));
        }
        Focus::PointerMaps => {
            lines.push(format!(
                "Pointer maps applicable: {}; complete: {}",
                graph.pointer_map.applicable, graph.pointer_map.complete
            ));
            lines.extend(
                graph
                    .pointer_map
                    .diagnostics
                    .iter()
                    .map(|code| format!("Diagnostic: {code}")),
            );
        }
        Focus::Diagnostics => {
            lines.extend(graph.diagnostics.iter().map(|finding| {
                format!(
                    "{} | {:?} | {:?}",
                    finding.code, finding.severity, finding.containment
                )
            }));
            lines.extend(
                graph
                    .schema
                    .diagnostics
                    .iter()
                    .map(|code| format!("Schema: {code}")),
            );
            for sidecar in &graph.sidecars {
                lines.extend(sidecar.diagnostics.iter().map(|finding| {
                    format!("{}: {} at {}", sidecar.kind, finding.code, finding.offset)
                }));
            }
            if lines.is_empty() {
                lines.push("No diagnostics in this revision".into());
            }
        }
        Focus::Help => unreachable!(),
    }
    lines
}

fn coordinates(evidence: &PhysicalEvidence) -> String {
    format!(
        "Evidence page {} bytes [{}, {}) | file offset {} | {}",
        evidence.page.page_number,
        evidence.range.page_offset,
        evidence.range.page_offset + evidence.range.length,
        evidence.range.file_offset,
        evidence.validation_rule
    )
}

fn object_lines(
    graph: &InspectionGraph,
    identity: &crate::inspection::CellIdentity,
) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(object) = graph
        .schema
        .objects
        .iter()
        .find(|object| &object.identity == identity)
    {
        lines.push(format!(
            "Schema cell {}:{} | {:?} | {:?}",
            identity.page_number, identity.index, object.object_type, object.state
        ));
        lines.push(format!(
            "Root claim: {}",
            object.root_page.as_deref().unwrap_or("NULL")
        ));
        lines.push(
            object
                .declaration
                .clone()
                .unwrap_or_else(|| "Declaration unavailable".into()),
        );
        if object.root.is_none() {
            lines.push(
                "Rootless or unavailable attribution: no directly attributable storage B-tree"
                    .into(),
            );
        }
        lines.extend(
            object
                .diagnostics
                .iter()
                .map(|code| format!("Diagnostic: {code}")),
        );
        lines.extend(object.evidence.iter().map(coordinates));
    }

    lines
}

fn page_lines(graph: &InspectionGraph, number: u32, focus: &Focus) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(page) = graph.pages.iter().find(|page| page.number == number) {
        lines.push(format!(
            "Page {} | {:?} | coverage {:?}",
            number, page.detail.kind, page.detail.coverage
        ));
        lines.extend(page_header_lines(page));
        if let Focus::Cell(identity) = focus {
            lines.extend(cell_lines(page, identity));
        }
        lines.extend(
            page.detail
                .diagnostics
                .iter()
                .map(|code| format!("Diagnostic: {code}")),
        );
        lines.push(format!("Reconciled role: {:?}", page.classification.role));
        if let Some(map) = graph
            .pointer_map
            .pages
            .iter()
            .find(|map| map.page.page_number == number)
        {
            for entry in &map.entries {
                lines.push(format!(
                    "Pointer-map target {:?} | {:?} | parent {:?} | {:?}",
                    entry.target, entry.kind, entry.parent_value, entry.state
                ));
                lines.push(coordinates(&entry.evidence));
                lines.extend(
                    entry
                        .diagnostics
                        .iter()
                        .map(|code| format!("Diagnostic: {code}")),
                );
            }
        }
        for region in &page.detail.regions {
            lines.push(format!(
                "{}: page bytes [{}, {}) | file offset {}",
                region.kind,
                region.range.page_offset,
                region.range.page_offset + region.range.length,
                region.range.file_offset
            ));
        }
        for claim in &page.classification.claims {
            lines.push(format!(
                "Role {:?} | {:?} | {}",
                claim.role, claim.state, claim.source
            ));
            lines.push(coordinates(&claim.evidence));
        }
        for relation in graph
            .relationships
            .iter()
            .filter(|r| entity_page(&r.source) == number || r.target.page_number == number)
        {
            lines.push(format!(
                "{:?}: {:?} -> Page {}",
                relation.kind, relation.source, relation.target.page_number
            ));
        }
        for diagnostic in graph
            .diagnostics
            .iter()
            .filter(|d| d.evidence.iter().any(|e| e.page.page_number == number))
        {
            lines.push(format!(
                "Diagnostic: {} ({:?})",
                diagnostic.code, diagnostic.severity
            ));
        }
    } else {
        lines.push("Invalid selector: page unavailable in this revision".into());
    }

    lines
}

fn cell_lines(
    page: &crate::inspection::PageEntity,
    identity: &crate::inspection::CellIdentity,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(cell) = page
        .detail
        .cells
        .iter()
        .find(|cell| &cell.identity == identity)
    {
        lines.push(format!(
            "Cell {}:{} | Offset: {}",
            identity.page_number, identity.index, cell.offset
        ));
        lines.push(format!(
            "Payload length: {:?}; rowid: {}",
            cell.payload_size,
            cell.rowid.as_deref().unwrap_or("unavailable")
        ));
        lines.push(format!(
            "Record state: {}",
            cell.record.as_ref().map_or_else(
                || "unavailable".into(),
                |record| format!("{:?}", record.state)
            )
        ));
        lines.push(format!(
            "Serial types: {:?}",
            cell.record.as_ref().map(|record| &record.serial_types)
        ));
        lines.push(format!(
            "Cell pointer: page bytes [{}, {}) | file offset {}",
            cell.pointer.page_offset,
            cell.pointer.page_offset + cell.pointer.length,
            cell.pointer.file_offset
        ));
        if let Some(range) = &cell.range {
            lines.push(format!(
                "Cell bytes [{}, {}) | file offset {}",
                range.page_offset,
                range.page_offset + range.length,
                range.file_offset
            ));
        }
        if let Some(code) = cell.diagnostic {
            lines.push(format!("Diagnostic: {code}"));
        }
        lines.push("Press d for explicit typed values; c hides values and cancels work.".into());
    } else {
        lines.push("Invalid selector: cell unavailable in this revision".into());
    }
    lines
}

fn page_header_lines(page: &crate::inspection::PageEntity) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(header) = &page.detail.header {
        lines.push(format!(
            "Cells declared: {} | Content start: {} | Fragmented bytes: {}",
            header.cell_count, header.content_start, header.fragmented_bytes
        ));
        lines.push(format!(
            "First freeblock: {} | Rightmost child claim: {:?}",
            header.first_freeblock, header.rightmost_child
        ));
    }
    for block in &page.detail.freeblocks {
        lines.push(format!("Freeblock at {} -> {}", block.offset, block.next));
        if let Some(range) = &block.range {
            lines.push(format!(
                "Freeblock bytes [{}, {}) | file offset {}",
                range.page_offset,
                range.page_offset + range.length,
                range.file_offset
            ));
        }
        if let Some(code) = block.diagnostic {
            lines.push(format!("Diagnostic: {code}"));
        }
    }
    lines
}
