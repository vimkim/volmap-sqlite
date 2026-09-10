//! Direct schema-record interpretation. This projection never changes physical facts.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::os::unix::fs::FileExt;

use serde::Serialize;

use super::topology::{Topology, WorkControl};
use super::{
    BtreeKind, CellDetail, CellIdentity, DatabaseGeometry, EntityIdentity, LocalCoverage,
    PageEntity, PageIdentity, PageRole, PhysicalEvidence, RelationshipKind, TextEncoding,
    TopologyCoverageReason, TraversalKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaState {
    Complete,
    Partial,
    Unavailable,
    DeclarationOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaObjectType {
    Table,
    Index,
    View,
    Trigger,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObject {
    /// Identity comes from the physical schema cell, never the name or rowid.
    pub identity: CellIdentity,
    pub evidence: Vec<PhysicalEvidence>,
    pub object_type: Option<SchemaObjectType>,
    pub name: Option<String>,
    pub table_name: Option<String>,
    /// Decimal string retains invalid signed claims without JavaScript rounding.
    pub root_page: Option<String>,
    pub declaration: Option<String>,
    pub root: Option<PageIdentity>,
    pub pages: Vec<PageIdentity>,
    pub state: SchemaState,
    pub diagnostics: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaEvidence {
    pub state: SchemaState,
    pub objects: Vec<SchemaObject>,
    pub diagnostics: Vec<&'static str>,
    pub max_decoded_bytes: String,
    pub decoded_bytes: String,
    pub stopping_cell: Option<CellIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaBudget {
    pub max_decoded_bytes: u64,
}

impl Default for SchemaBudget {
    fn default() -> Self {
        Self {
            max_decoded_bytes: 16 * 1024 * 1024,
        }
    }
}

impl SchemaEvidence {
    pub(super) fn unavailable(budget: SchemaBudget) -> Self {
        Self {
            state: SchemaState::Unavailable,
            objects: Vec::new(),
            diagnostics: vec!["schema_btree_unavailable"],
            max_decoded_bytes: budget.max_decoded_bytes.to_string(),
            decoded_bytes: "0".into(),
            stopping_cell: None,
        }
    }
}

#[derive(Default)]
struct Tree {
    pages: BTreeSet<u32>,
    partial: bool,
}

struct TreeIndex {
    roots: BTreeMap<u32, Tree>,
    child_targets: BTreeSet<u32>,
}

fn source_page(entity: &EntityIdentity) -> u32 {
    match entity {
        EntityIdentity::Page { page_number } | EntityIdentity::Cell { page_number, .. } => {
            *page_number
        }
    }
}

fn btree_role(topology: &Topology, number: u32) -> Option<PageRole> {
    topology
        .classifications
        .get(number.checked_sub(1)? as usize)
        .filter(|classification| classification.reconciled)
        .map(|classification| classification.role)
        .filter(|role| {
            matches!(
                role,
                PageRole::TableLeaf
                    | PageRole::TableInterior
                    | PageRole::IndexLeaf
                    | PageRole::IndexInterior
            )
        })
}

/// Reuse bounded traversal prefixes and check their links against final reconciliation.
fn index_trees(
    topology: &Topology,
    pages: &[PageEntity],
    control: &WorkControl,
    checkpoint: &mut impl FnMut(),
) -> Result<TreeIndex, &'static str> {
    let mut units = 0_u64;
    let mut step = || {
        if units.is_multiple_of(256) {
            checkpoint();
        }
        units += 1;
        check_stop(control)
    };
    step()?;
    let mut links = BTreeSet::new();
    for link in &topology.relationships {
        step()?;
        if link.kind == RelationshipKind::BtreeChild {
            links.insert((source_page(&link.source), link.target.page_number));
        }
    }
    let mut child_targets = BTreeSet::new();
    for claim in &topology.claims {
        step()?;
        if claim.kind == RelationshipKind::BtreeChild
            && let Some(target) = &claim.target
        {
            child_targets.insert(target.page_number);
        }
    }
    let mut result = BTreeMap::<u32, Tree>::new();
    for traversal in &topology.traversals {
        step()?;
        if traversal.kind != TraversalKind::Btree {
            continue;
        }
        let root = source_page(&traversal.origin);
        let tree = result.entry(root).or_default();
        tree.partial |= traversal.stop.is_some()
            || topology.coverage.reason != TopologyCoverageReason::Complete;
        let mut previous = None;
        for page in &traversal.validated_prefix {
            step()?;
            let number = page.page_number;
            if btree_role(topology, number).is_none()
                || previous.is_some_and(|parent| !links.contains(&(parent, number)))
            {
                tree.partial = true;
                break;
            }
            tree.pages.insert(number);
            tree.partial |= pages[(number - 1) as usize].detail.coverage != LocalCoverage::Complete;
            previous = Some(number);
        }
    }
    Ok(TreeIndex {
        roots: result,
        child_targets,
    })
}

fn check_stop(control: &WorkControl) -> Result<(), &'static str> {
    match control.traversal_reason() {
        Some(super::TraversalStopReason::OperatorStop) => Err("schema_operator_stop"),
        Some(super::TraversalStopReason::Budget) => Err("schema_work_budget"),
        Some(_) => Err("schema_cancelled"),
        None => {
            control.advance(super::TopologyPhase::SchemaInspection);
            Ok(())
        }
    }
}

pub(super) fn inspect(
    file: &File,
    geometry: &DatabaseGeometry,
    pages: &[PageEntity],
    topology: &Topology,
    control: &WorkControl,
    budget: SchemaBudget,
    checkpoint: &mut impl FnMut(),
) -> SchemaEvidence {
    control.begin_phase(super::TopologyPhase::SchemaInspection, None);
    let trees = match index_trees(topology, pages, control, checkpoint) {
        Ok(trees) => trees,
        Err(code) => {
            let mut evidence = SchemaEvidence::unavailable(budget);
            evidence.state = SchemaState::Partial;
            evidence.diagnostics = vec![code];
            return evidence;
        }
    };
    let Some(schema_tree) = trees.roots.get(&1).filter(|tree| tree.pages.contains(&1)) else {
        control.mark_unavailable();
        return SchemaEvidence::unavailable(budget);
    };
    let mut result = SchemaEvidence {
        state: SchemaState::Complete,
        objects: vec![],
        diagnostics: vec![],
        max_decoded_bytes: budget.max_decoded_bytes.to_string(),
        decoded_bytes: "0".into(),
        stopping_cell: None,
    };
    let mut reader = PayloadReader {
        file,
        geometry,
        topology,
        control,
        remaining: budget.max_decoded_bytes,
    };
    if schema_tree.partial {
        result.state = SchemaState::Partial;
        result.diagnostics.push("schema_btree_partial");
    }
    'pages: for number in &schema_tree.pages {
        if control.traversal_reason().is_some() {
            result.state = SchemaState::Partial;
            result.diagnostics.push(check_stop(control).unwrap_err());
            break;
        }
        let page = &pages[(*number - 1) as usize];
        if page.detail.kind != Some(BtreeKind::TableLeaf) {
            continue;
        }
        for cell in &page.detail.cells {
            checkpoint();
            if control.traversal_reason().is_some()
                || cell
                    .payload_size
                    .is_some_and(|size| size > reader.remaining)
            {
                result.state = SchemaState::Partial;
                result
                    .diagnostics
                    .push(if control.traversal_reason().is_some() {
                        check_stop(control).unwrap_err()
                    } else {
                        "schema_decode_budget"
                    });
                result.stopping_cell = Some(cell.identity.clone());
                break 'pages;
            }
            let object = decode(&mut reader, cell);
            if !object.diagnostics.is_empty() {
                result.state = SchemaState::Partial;
            }
            result.objects.push(object);
        }
    }
    result.decoded_bytes = (budget.max_decoded_bytes - reader.remaining).to_string();
    reconcile_attribution(&mut result, topology, &trees, control);
    if result.diagnostics.contains(&"schema_decode_budget")
        || result
            .objects
            .iter()
            .any(|object| object.diagnostics.contains(&"schema_decode_budget"))
    {
        control.mark_local_budget(super::BudgetKind::SchemaDecodedBytes);
    } else if control.traversal_reason().is_none() {
        control.finish_phase(super::TopologyPhase::SchemaInspection);
    }
    result
}

fn reconcile_attribution(
    result: &mut SchemaEvidence,
    topology: &Topology,
    trees: &TreeIndex,
    control: &WorkControl,
) {
    let mut root_counts = BTreeMap::<u32, usize>::new();
    for object in &result.objects {
        if let Err(code) = check_stop(control) {
            result.state = SchemaState::Partial;
            if !result.diagnostics.contains(&code) {
                result.diagnostics.push(code);
            }
            return;
        }
        if let Some(root) = object
            .root_page
            .as_deref()
            .and_then(|root| root.parse::<u32>().ok())
            .filter(|root| *root > 1)
        {
            *root_counts.entry(root).or_default() += 1;
        }
    }
    for object in &mut result.objects {
        if let Err(code) = check_stop(control) {
            result.state = SchemaState::Partial;
            if !result.diagnostics.contains(&code) {
                result.diagnostics.push(code);
            }
            return;
        }
        if object
            .root_page
            .as_deref()
            .and_then(|root| root.parse::<u32>().ok())
            .is_some_and(|root| root_counts.get(&root).is_some_and(|count| *count > 1))
        {
            object.state = SchemaState::Unavailable;
            object.diagnostics.push("schema_root_conflicting");
            result.state = SchemaState::Partial;
            continue;
        }
        attribute(object, topology, trees, control);
        if matches!(
            object.state,
            SchemaState::Partial | SchemaState::Unavailable
        ) {
            result.state = SchemaState::Partial;
        }
    }
}

fn decode(reader: &mut PayloadReader<'_>, cell: &CellDetail) -> SchemaObject {
    let mut object = SchemaObject {
        identity: cell.identity.clone(),
        evidence: vec![PhysicalEvidence {
            page: PageIdentity {
                page_number: cell.identity.page_number,
            },
            range: cell.local_payload.as_ref().unwrap_or(&cell.pointer).clone(),
            validation_rule: "sqlite_schema_record",
        }],
        object_type: None,
        name: None,
        table_name: None,
        root_page: None,
        declaration: None,
        root: None,
        pages: vec![],
        state: SchemaState::Unavailable,
        diagnostics: vec![],
    };
    let decoded = reader
        .read(cell, &mut object.evidence)
        .and_then(|bytes| record(&bytes, reader.geometry));
    match decoded {
        Ok(record) => {
            object.object_type = Some(record.kind);
            object.name = Some(record.name);
            object.table_name = Some(record.table);
            object.root_page = record.root.map(|root| root.to_string());
            object.declaration = record.sql;
        }
        Err(code) => object.diagnostics.push(code),
    }
    object
}

fn attribute(
    object: &mut SchemaObject,
    topology: &Topology,
    trees: &TreeIndex,
    control: &WorkControl,
) {
    let Some(kind) = object.object_type else {
        return;
    };
    let root = object
        .root_page
        .as_deref()
        .and_then(|root| root.parse::<u32>().ok());
    let virtual_declaration =
        kind == SchemaObjectType::Table && object.declaration.as_deref().is_some_and(virtual_table);
    if object.root_page.as_deref().is_none_or(|root| root == "0")
        && (matches!(kind, SchemaObjectType::View | SchemaObjectType::Trigger)
            || virtual_declaration)
    {
        object.state = SchemaState::DeclarationOnly;
        return;
    }
    if virtual_declaration {
        object.diagnostics.push("schema_virtual_root_invalid");
        return;
    }
    let valid_root = root
        .filter(|root| *root > 1)
        .filter(|root| {
            btree_role(topology, *root).is_some_and(|role| {
                kind == SchemaObjectType::Table
                    || (kind == SchemaObjectType::Index
                        && matches!(role, PageRole::IndexLeaf | PageRole::IndexInterior))
            })
        })
        .filter(|root| !trees.child_targets.contains(root));
    if let Err(code) = check_stop(control) {
        object.diagnostics.push(code);
        return;
    }
    let Some((root, tree)) = valid_root.and_then(|root| {
        trees
            .roots
            .get(&root)
            .filter(|tree| tree.pages.contains(&root))
            .map(|tree| (root, tree))
    }) else {
        object.diagnostics.push("schema_root_unavailable");
        return;
    };
    object.root = Some(PageIdentity { page_number: root });
    for number in &tree.pages {
        if let Err(code) = check_stop(control) {
            object.state = SchemaState::Partial;
            object.diagnostics.push(code);
            return;
        }
        object.pages.push(PageIdentity {
            page_number: *number,
        });
    }
    object.state = if tree.partial {
        SchemaState::Partial
    } else {
        SchemaState::Complete
    };
    if tree.partial {
        object.diagnostics.push("schema_attribution_partial");
    }
}

struct Record {
    kind: SchemaObjectType,
    name: String,
    table: String,
    root: Option<i64>,
    sql: Option<String>,
}

fn record(bytes: &[u8], geometry: &DatabaseGeometry) -> Result<Record, &'static str> {
    let mut cursor = 0;
    let header = usize::try_from(super::btree::varint(bytes, &mut cursor)?)
        .map_err(|_| "schema_record_invalid")?;
    if header < cursor || header > bytes.len() {
        return Err("schema_record_invalid");
    }
    let mut serials = [0; 5];
    for serial in &mut serials {
        *serial = super::btree::varint(&bytes[..header], &mut cursor)?;
    }
    if cursor != header {
        return Err("schema_record_invalid");
    }
    let mut fields = Vec::new();
    for serial in serials {
        let length = match serial {
            0 | 8 | 9 => 0,
            1..=4 => serial,
            5 => 6,
            6 => 8,
            value if value >= 13 && value % 2 == 1 => (value - 13) / 2,
            _ => return Err("schema_record_invalid"),
        };
        if matches!(serial, 8 | 9) && geometry.schema_format < 4 {
            return Err("schema_record_invalid");
        }
        let end = cursor
            .checked_add(usize::try_from(length).map_err(|_| "schema_record_invalid")?)
            .ok_or("schema_record_invalid")?;
        let field = bytes.get(cursor..end).ok_or("schema_record_invalid")?;
        fields.push((serial, field));
        cursor = end;
    }
    if cursor != bytes.len() {
        return Err("schema_record_invalid");
    }
    let text = |index: usize| -> Result<String, &'static str> {
        let (serial, bytes) = fields[index];
        if serial < 13 || serial % 2 == 0 {
            return Err("schema_record_invalid");
        }
        let text = decode_text(bytes, geometry.text_encoding)?;
        if text.contains('\0') {
            return Err("schema_text_invalid");
        }
        Ok(text)
    };
    let kind = match text(0)?.as_str() {
        "table" => SchemaObjectType::Table,
        "index" => SchemaObjectType::Index,
        "view" => SchemaObjectType::View,
        "trigger" => SchemaObjectType::Trigger,
        _ => return Err("schema_object_type_invalid"),
    };
    let name = text(1)?;
    let table = text(2)?;
    if name.is_empty() || table.is_empty() {
        return Err("schema_record_invalid");
    }
    let (serial, bytes) = fields[3];
    let root = match serial {
        0 => None,
        8 => Some(0),
        9 => Some(1),
        1..=6 => {
            let mut value = [if bytes[0] & 0x80 == 0 { 0 } else { 255 }; 8];
            value[8 - bytes.len()..].copy_from_slice(bytes);
            Some(i64::from_be_bytes(value))
        }
        _ => return Err("schema_record_invalid"),
    };
    let sql = if fields[4].0 == 0 {
        None
    } else {
        Some(text(4)?)
    };
    if sql.is_none() && kind != SchemaObjectType::Index {
        return Err("schema_declaration_unavailable");
    }
    Ok(Record {
        kind,
        name,
        table,
        root,
        sql,
    })
}

pub(super) fn decode_text(bytes: &[u8], encoding: TextEncoding) -> Result<String, &'static str> {
    let text = match encoding {
        TextEncoding::Utf8 => {
            String::from_utf8(bytes.to_vec()).map_err(|_| "schema_text_invalid")?
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            if !bytes.len().is_multiple_of(2) {
                return Err("schema_text_invalid");
            }
            let words: Vec<_> = bytes
                .chunks_exact(2)
                .map(|pair| {
                    let pair = [pair[0], pair[1]];
                    if encoding == TextEncoding::Utf16Le {
                        u16::from_le_bytes(pair)
                    } else {
                        u16::from_be_bytes(pair)
                    }
                })
                .collect();
            String::from_utf16(&words).map_err(|_| "schema_text_invalid")?
        }
    };
    Ok(text)
}

struct PayloadReader<'a> {
    file: &'a File,
    geometry: &'a DatabaseGeometry,
    topology: &'a Topology,
    control: &'a WorkControl,
    remaining: u64,
}

impl PayloadReader<'_> {
    fn read(
        &mut self,
        cell: &CellDetail,
        evidence: &mut Vec<PhysicalEvidence>,
    ) -> Result<Vec<u8>, &'static str> {
        if cell.diagnostic.is_some() || !(1..=4).contains(&self.geometry.schema_format) {
            return Err("schema_record_invalid");
        }
        let range = cell
            .local_payload
            .as_ref()
            .ok_or("schema_payload_unavailable")?;
        let size = cell.payload_size.ok_or("schema_payload_unavailable")?;
        if !self.control.reserve_memory(size.saturating_mul(8)) {
            return Err("schema_work_budget");
        }
        self.remaining = self
            .remaining
            .checked_sub(size)
            .ok_or("schema_decode_budget")?;
        let size = usize::try_from(size).map_err(|_| "schema_allocation_failed")?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| "schema_allocation_failed")?;
        bytes.resize(size, 0);
        let local = range.length as usize;
        self.file
            .read_exact_at(
                bytes.get_mut(..local).ok_or("schema_record_invalid")?,
                range.file_offset,
            )
            .map_err(|_| "schema_read_failed")?;
        if local == size {
            return Ok(bytes);
        }
        let origin = EntityIdentity::Cell {
            page_number: cell.identity.page_number,
            cell_index: cell.identity.index,
        };
        let traversal = self
            .topology
            .traversals
            .iter()
            .take_while(|_| check_stop(self.control).is_ok())
            .find(|traversal| {
                traversal.kind == TraversalKind::Overflow && traversal.origin == origin
            })
            .filter(|traversal| traversal.stop.is_none())
            .ok_or("schema_overflow_unavailable")?;
        let mut cursor = local;
        let mut source = origin;
        for page in &traversal.validated_prefix {
            if self.control.traversal_reason().is_some() {
                return Err("schema_cancelled");
            }
            if !self
                .topology
                .classifications
                .get((page.page_number - 1) as usize)
                .is_some_and(|classification| {
                    classification.reconciled && classification.role == PageRole::Overflow
                })
                || !self
                    .topology
                    .relationships
                    .iter()
                    .take_while(|_| check_stop(self.control).is_ok())
                    .any(|link| {
                        link.kind == RelationshipKind::Overflow
                            && link.source == source
                            && link.target == *page
                    })
            {
                return Err("schema_overflow_unavailable");
            }
            let length = (size - cursor).min((self.geometry.usable_size - 4) as usize);
            if length == 0 {
                return Err("schema_overflow_unavailable");
            }
            let extent = super::roles::evidence(
                self.geometry,
                page.page_number,
                4,
                u32::try_from(length).map_err(|_| "schema_record_invalid")?,
                "sqlite_schema_overflow_payload",
            );
            self.file
                .read_exact_at(
                    &mut bytes[cursor..cursor + length],
                    extent.range.file_offset,
                )
                .map_err(|_| "schema_read_failed")?;
            evidence.push(extent);
            cursor += length;
            source = EntityIdentity::Page {
                page_number: page.page_number,
            };
        }
        if cursor != size {
            return Err("schema_overflow_unavailable");
        }
        Ok(bytes)
    }
}

// Only recognize the declaration prefix needed for a rootless virtual table.
// This is not a SQL evaluator or a general declaration parser.
fn virtual_table(mut sql: &str) -> bool {
    for keyword in ["CREATE", "VIRTUAL", "TABLE"] {
        loop {
            sql = sql.trim_start();
            if let Some(comment) = sql.strip_prefix("/*") {
                let Some((_, rest)) = comment.split_once("*/") else {
                    return false;
                };
                sql = rest;
            } else if let Some(comment) = sql.strip_prefix("--") {
                let Some((_, rest)) = comment.split_once('\n') else {
                    return false;
                };
                sql = rest;
            } else {
                break;
            }
        }
        let length = sql.bytes().take_while(u8::is_ascii_alphabetic).count();
        if !sql[..length].eq_ignore_ascii_case(keyword) {
            return false;
        }
        sql = &sql[length..];
        if sql.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}
