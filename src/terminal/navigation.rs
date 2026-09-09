use super::{Entry, Focus, TerminalFlow, entity_page};
use crate::inspection::EntityIdentity;
impl TerminalFlow {
    pub(super) fn entries(&self) -> Vec<Entry> {
        let Some(graph) = &self.graph else {
            return Vec::new();
        };
        match self.focus() {
            Focus::Database => [
                ("Schema objects", Focus::Schema),
                ("B-tree storage", Focus::Btrees),
                ("Freelist", Focus::Freelist),
                ("Pointer maps", Focus::PointerMaps),
                ("Diagnostics", Focus::Diagnostics),
            ]
            .into_iter()
            .map(|(label, focus)| Entry {
                label: label.into(),
                focus,
            })
            .collect(),
            Focus::Schema => graph
                .schema
                .objects
                .iter()
                .map(|object| Entry {
                    label: object
                        .name
                        .clone()
                        .unwrap_or_else(|| "Unavailable schema record".into()),
                    focus: Focus::Object(object.identity.clone()),
                })
                .collect(),
            Focus::Object(identity) => graph
                .schema
                .objects
                .iter()
                .find(|object| &object.identity == identity)
                .map_or_else(Vec::new, |object| {
                    object
                        .pages
                        .iter()
                        .map(|page| page_entry(page.page_number))
                        .collect()
                }),
            Focus::Btrees => graph
                .pages
                .iter()
                .filter(|page| {
                    matches!(
                        page.classification.role,
                        crate::inspection::PageRole::Btree
                            | crate::inspection::PageRole::TableLeaf
                            | crate::inspection::PageRole::TableInterior
                            | crate::inspection::PageRole::IndexLeaf
                            | crate::inspection::PageRole::IndexInterior
                    )
                })
                .map(|page| page_entry(page.number))
                .collect(),
            Focus::Freelist => graph
                .pages
                .iter()
                .filter(|page| page.detail.allocation_role.is_some())
                .map(|page| page_entry(page.number))
                .collect(),
            Focus::PointerMaps => graph
                .pointer_map
                .locations
                .iter()
                .map(|page| page_entry(page.page_number))
                .collect(),
            Focus::Page(number) => page_entries(graph, *number),
            Focus::Cell(identity) => graph
                .relationships
                .iter()
                .filter(|relation| {
                    relation.source
                        == EntityIdentity::Cell {
                            page_number: identity.page_number,
                            cell_index: identity.index,
                        }
                })
                .map(|relation| page_entry(relation.target.page_number))
                .collect(),
            Focus::Diagnostics => graph
                .diagnostics
                .iter()
                .flat_map(|finding| finding.evidence.iter())
                .map(|evidence| page_entry(evidence.page.page_number))
                .collect(),
            Focus::Help => Vec::new(),
        }
    }
}

fn page_entries(graph: &crate::inspection::InspectionGraph, number: u32) -> Vec<Entry> {
    graph
        .pages
        .iter()
        .find(|page| page.number == number)
        .map_or_else(Vec::new, |page| {
            let mut entries: Vec<_> = page
                .detail
                .cells
                .iter()
                .map(|cell| Entry {
                    label: format!("Cell {}", cell.identity.index),
                    focus: Focus::Cell(cell.identity.clone()),
                })
                .collect();
            if let Some(map) = graph
                .pointer_map
                .pages
                .iter()
                .find(|map| map.page.page_number == number)
            {
                entries.extend(map.entries.iter().filter_map(|entry| {
                    let target = entity_page(&entry.target);
                    graph
                        .pages
                        .iter()
                        .any(|page| page.number == target)
                        .then(|| page_entry(target))
                }));
            }
            for relation in &graph.relationships {
                if entity_page(&relation.source) == number {
                    entries.push(page_entry(relation.target.page_number));
                }
            }
            entries
        })
}

fn page_entry(number: u32) -> Entry {
    Entry {
        label: format!("Page {number}"),
        focus: Focus::Page(number),
    }
}
