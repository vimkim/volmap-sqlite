use super::{Entry, Focus, TerminalFlow, entity_page, window};
use crate::inspection::EntityIdentity;
impl TerminalFlow {
    pub(super) fn entries(&self) -> Vec<Entry> {
        let Some(graph) = &self.graph else {
            return Vec::new();
        };
        let mut entries = match self.focus() {
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
                .enumerate()
                .map(|(index, object)| Entry {
                    label: object
                        .name
                        .clone()
                        .unwrap_or_else(|| "Unavailable schema record".into()),
                    focus: Focus::Object(object.identity.clone(), self.location().offset + index),
                })
                .collect(),
            Focus::Object(identity, _) => graph
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
                .freelist
                .trunks
                .iter()
                .map(|trunk| page_entry(trunk.page.page_number))
                .collect(),
            Focus::PointerMaps => graph
                .pointer_map
                .locations
                .iter()
                .map(|page| page_entry(page.page_number))
                .collect(),
            Focus::Page(number) => page_entries(graph, *number, self.location().offset),
            Focus::Traversal(_) => prefix_entries(graph),
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
            Focus::Help | Focus::WindowOffset(_) => Vec::new(),
        };
        self.add_page_schema_entries(&mut entries);
        self.add_traversal_entries(&mut entries);
        self.add_window_entries(&mut entries);
        entries
    }

    fn add_traversal_entries(&self, entries: &mut Vec<Entry>) {
        for header in &self.traversal_headers {
            let visible = match self.focus() {
                Focus::Page(_) => true,
                Focus::Cell(cell) => {
                    header.origin
                        == EntityIdentity::Cell {
                            page_number: cell.page_number,
                            cell_index: cell.index,
                        }
                }
                _ => false,
            };
            if visible {
                entries.push(Entry {
                    label: format!(
                        "Traversal {:?} ({} pages)",
                        header.kind, header.prefix_count
                    ),
                    focus: Focus::Traversal(header.traversal_offset),
                });
            }
        }
    }

    fn add_page_schema_entries(&self, entries: &mut Vec<Entry>) {
        if !matches!(self.focus(), Focus::Page(_)) {
            return;
        }
        if let Some(graph) = &self.graph {
            entries.extend(graph.schema.objects.iter().zip(&self.schema_positions).map(
                |(object, position)| Entry {
                    label: format!("Schema {}", object.name.as_deref().unwrap_or("record")),
                    focus: Focus::Object(object.identity.clone(), *position),
                },
            ));
        }
    }

    fn add_window_entries(&self, entries: &mut Vec<Entry>) {
        if self.location().offset > 0 {
            entries.push(Entry {
                label: "Previous collection window".into(),
                focus: Focus::WindowOffset(
                    self.location().offset.saturating_sub(window::SIZE as usize),
                ),
            });
        }
        if let Some(next) = self.next_window() {
            entries.push(Entry {
                label: "Next collection window".into(),
                focus: Focus::WindowOffset(next),
            });
        }
    }
}

fn prefix_entries(graph: &crate::inspection::InspectionGraph) -> Vec<Entry> {
    graph
        .traversals
        .iter()
        .flat_map(|traversal| {
            traversal
                .validated_prefix
                .iter()
                .map(|page| page_entry(page.page_number))
        })
        .collect()
}

fn page_entries(
    graph: &crate::inspection::InspectionGraph,
    number: u32,
    offset: usize,
) -> Vec<Entry> {
    graph
        .pages
        .iter()
        .find(|page| page.number == number)
        .map_or_else(Vec::new, |page| {
            let mut entries: Vec<_> = page
                .detail
                .cells
                .iter()
                .skip(offset)
                .take(window::SIZE as usize)
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
                entries.extend(
                    map.entries
                        .iter()
                        .skip(offset)
                        .take(window::SIZE as usize)
                        .filter_map(|entry| {
                            let target = entity_page(&entry.target);
                            (target >= 1 && target <= graph.coverage.evaluated)
                                .then(|| page_entry(target))
                        }),
                );
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
