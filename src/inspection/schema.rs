//! Direct schema-record interpretation. This projection never changes physical facts.
use std::fs::File;
use std::os::unix::fs::FileExt;

use serde::Serialize;

use super::topology::{Topology, WorkControl};
use super::{
    BtreeKind, CellDetail, CellIdentity, DatabaseGeometry, EntityIdentity, LocalCoverage,
    PageIdentity, PageRole, PhysicalEvidence, RelationshipKind, TextEncoding,
    TopologyCoverageReason, TraversalKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaState {
    Complete,
    Partial,
    Unavailable,
    DeclarationOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaObjectType {
    Table,
    Index,
    View,
    Trigger,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
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
    #[serde(skip)]
    pub(super) attributed_through: Option<u32>,
    pub state: SchemaState,
    pub diagnostics: Vec<std::borrow::Cow<'static, str>>,
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

pub(super) struct StoredSchema {
    pub header: SchemaEvidence,
    pub objects: super::index::Sequence<SchemaObject>,
    members: super::index_map::IndexMap<(u32, u32), ()>,
    attributions: super::index_map::IndexMap<(u32, usize), ()>,
}
impl std::ops::Deref for StoredSchema {
    type Target = SchemaEvidence;
    fn deref(&self) -> &SchemaEvidence {
        &self.header
    }
}
impl std::ops::DerefMut for StoredSchema {
    fn deref_mut(&mut self) -> &mut SchemaEvidence {
        &mut self.header
    }
}
impl StoredSchema {
    pub(super) fn objects_for_page(&self, page: u32) -> impl Iterator<Item = usize> + '_ {
        self.attributions
            .keys_after(Some((page.saturating_sub(1), usize::MAX)))
            .take_while(move |(owner, _)| *owner == page)
            .map(|(_, position)| position)
    }
    pub(super) fn attributed_pages<'a>(
        &'a self,
        object: &'a SchemaObject,
    ) -> impl Iterator<Item = PageIdentity> + 'a {
        let root = object.root.as_ref().map_or(0, |root| root.page_number);
        let last = object.attributed_through.unwrap_or(0);
        self.members
            .keys_after(Some((root, 0)))
            .take_while(move |(owner, page)| *owner == root && *page <= last)
            .map(|(_, page_number)| PageIdentity { page_number })
    }

    fn storage_stop(&mut self, error: super::storage::StorageError) {
        self.header.state = SchemaState::Partial;
        let code = match error {
            super::storage::StorageError::Budget => "schema_work_budget",
            super::storage::StorageError::Unavailable => "private_storage_unavailable",
        };
        if !self.header.diagnostics.contains(&code) {
            self.header.diagnostics.push(code);
        }
    }

    pub(super) fn unavailable(budget: SchemaBudget, pages: &super::storage::PageStore) -> Self {
        Self {
            header: SchemaEvidence::unavailable(budget),
            objects: super::index::Sequence::new(std::sync::Arc::clone(&pages.context)),
            members: super::index_map::IndexMap::new(std::sync::Arc::clone(&pages.context)),
            attributions: super::index_map::IndexMap::new(std::sync::Arc::clone(&pages.context)),
        }
    }
    pub(super) fn estimated_total(&self) -> Result<u64, super::storage::StorageError> {
        Ok(self
            .objects
            .estimated_total()?
            .saturating_add(self.members.estimated_total()?))
    }
    pub(super) fn metadata(&self, limit: usize) -> SchemaEvidence {
        let mut evidence = self.header.clone();
        evidence.objects = self.objects.iter().take(limit).collect();
        evidence
    }
    pub(super) fn materialize(&self) -> Result<SchemaEvidence, super::storage::StorageError> {
        let mut evidence = self.header.clone();
        evidence.objects = self.objects.materialize()?;
        for object in &mut evidence.objects {
            if let (Some(root), Some(last)) = (&object.root, object.attributed_through) {
                object.pages = self
                    .members
                    .keys_after(Some((root.page_number, 0)))
                    .take_while(|(owner, page)| *owner == root.page_number && *page <= last)
                    .map(|(_, page_number)| PageIdentity { page_number })
                    .collect();
            }
        }
        self.members.check()?;
        Ok(evidence)
    }
}

struct TreeIndex {
    roots: super::index_map::IndexMap<u32, bool>,
    members: super::index_map::IndexMap<(u32, u32), ()>,
    child_targets: super::index_map::IndexMap<u32, ()>,
}

impl TreeIndex {
    fn pages(&self, root: u32) -> impl Iterator<Item = u32> + '_ {
        self.members
            .keys_after(Some((root, 0)))
            .take_while(move |(owner, _)| *owner == root)
            .map(|(_, page)| page)
    }
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
    pages: &super::storage::PageStore,
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
    let mut links = topology.claims.map();
    for link in &topology.relationships {
        step()?;
        if link.kind == RelationshipKind::BtreeChild {
            links.insert((source_page(&link.source), link.target.page_number), ());
        }
    }
    let mut child_targets = topology.claims.map();
    for claim in &topology.claims {
        step()?;
        if claim.kind == RelationshipKind::BtreeChild
            && let Some(target) = &claim.target
        {
            child_targets.insert(target.page_number, ());
        }
    }
    let mut result = topology.claims.map();
    let mut members = topology.claims.map();
    for traversal in &topology.traversals {
        step()?;
        if traversal.kind != TraversalKind::Btree {
            continue;
        }
        let root = source_page(&traversal.origin);
        let mut partial = result.get(&root).unwrap_or(false);
        partial |= traversal.stop.is_some()
            || topology.coverage.reason != TopologyCoverageReason::Complete;
        let mut previous = None;
        for page in &traversal.validated_prefix {
            step()?;
            let number = page.page_number;
            if btree_role(topology, number).is_none()
                || previous.is_some_and(|parent| !links.contains_key(&(parent, number)))
            {
                partial = true;
                break;
            }
            members.insert((root, number), ());
            partial |= pages
                .get((number - 1) as usize)
                .ok_or("private_storage_unavailable")?
                .detail
                .coverage
                != LocalCoverage::Complete;
            previous = Some(number);
        }
        result.insert(root, partial);
    }
    Ok(TreeIndex {
        roots: result,
        members,
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
    pages: &super::storage::PageStore,
    topology: &Topology,
    control: &WorkControl,
    budget: SchemaBudget,
    checkpoint: &mut impl FnMut(),
) -> StoredSchema {
    control.begin_phase(super::TopologyPhase::SchemaInspection, None);
    let trees = match index_trees(topology, pages, control, checkpoint) {
        Ok(trees) => trees,
        Err(code) => {
            let mut evidence = StoredSchema::unavailable(budget, pages);
            evidence.state = SchemaState::Partial;
            evidence.diagnostics = vec![code];
            return evidence;
        }
    };
    let Some(schema_partial) = trees
        .roots
        .get(&1)
        .filter(|_| trees.members.contains_key(&(1, 1)))
    else {
        control.mark_unavailable();
        return StoredSchema::unavailable(budget, pages);
    };
    let mut result = StoredSchema::unavailable(budget, pages);
    result.header = SchemaEvidence {
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
    if schema_partial {
        result.state = SchemaState::Partial;
        result.diagnostics.push("schema_btree_partial");
    }
    'pages: for number in trees.pages(1) {
        if control.traversal_reason().is_some() {
            result.state = SchemaState::Partial;
            result.diagnostics.push(check_stop(control).unwrap_err());
            break;
        }
        let Some(page) = pages.get((number - 1) as usize) else {
            result.state = SchemaState::Unavailable;
            break;
        };
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
            if let Err(error) = pages.check() {
                result.storage_stop(error);
                result.stopping_cell = Some(cell.identity.clone());
                break 'pages;
            }
        }
    }
    result.decoded_bytes = (budget.max_decoded_bytes - reader.remaining).to_string();
    reconcile_attribution(&mut result, topology, &trees, control);
    if let Err(error) = pages.check() {
        result.storage_stop(error);
    }
    if result.diagnostics.contains(&"schema_decode_budget")
        || result
            .objects
            .iter()
            .any(|object| object.diagnostics.contains(&"schema_decode_budget".into()))
    {
        control.mark_local_budget(super::BudgetKind::SchemaDecodedBytes);
    } else if control.traversal_reason().is_none() {
        control.finish_phase(super::TopologyPhase::SchemaInspection);
    }
    result.members = trees.members;
    result
}

fn reconcile_attribution(
    result: &mut StoredSchema,
    topology: &Topology,
    trees: &TreeIndex,
    control: &WorkControl,
) {
    let mut root_counts = topology.claims.map();
    for object in &result.objects {
        if let Err(code) = check_stop(control) {
            result.header.state = SchemaState::Partial;
            if !result.header.diagnostics.contains(&code) {
                result.header.diagnostics.push(code);
            }
            return;
        }
        if let Some(root) = object
            .root_page
            .as_deref()
            .and_then(|root| root.parse::<u32>().ok())
            .filter(|root| *root > 1)
        {
            let count: usize = root_counts.get(&root).unwrap_or(0);
            root_counts.insert(root, count.saturating_add(1));
        }
    }
    for index in 0..result.objects.len() {
        let Some(mut object) = result.objects.get_mut(index) else {
            return;
        };
        if let Err(code) = check_stop(control) {
            result.header.state = SchemaState::Partial;
            if !result.header.diagnostics.contains(&code) {
                result.header.diagnostics.push(code);
            }
            return;
        }
        if object
            .root_page
            .as_deref()
            .and_then(|root| root.parse::<u32>().ok())
            .is_some_and(|root| root_counts.get(&root).is_some_and(|count| count > 1))
        {
            object.state = SchemaState::Unavailable;
            object.diagnostics.push(("schema_root_conflicting").into());
            result.header.state = SchemaState::Partial;
            continue;
        }
        attribute(
            &mut object,
            topology,
            trees,
            control,
            index,
            &mut result.attributions,
        );
        if matches!(
            object.state,
            SchemaState::Partial | SchemaState::Unavailable
        ) {
            result.header.state = SchemaState::Partial;
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
            validation_rule: std::borrow::Cow::Borrowed("sqlite_schema_record"),
        }],
        object_type: None,
        name: None,
        table_name: None,
        root_page: None,
        declaration: None,
        root: None,
        pages: vec![],
        attributed_through: None,
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
        Err(code) => object.diagnostics.push((code).into()),
    }
    object
}

fn attribute(
    object: &mut SchemaObject,
    topology: &Topology,
    trees: &TreeIndex,
    control: &WorkControl,
    position: usize,
    attributions: &mut super::index_map::IndexMap<(u32, usize), ()>,
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
        object
            .diagnostics
            .push(("schema_virtual_root_invalid").into());
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
        .filter(|root| !trees.child_targets.contains_key(root));
    if let Err(code) = check_stop(control) {
        object.diagnostics.push((code).into());
        return;
    }
    let Some((root, partial)) = valid_root.and_then(|root| {
        trees
            .roots
            .get(&root)
            .filter(|_| trees.members.contains_key(&(root, root)))
            .map(|tree| (root, tree))
    }) else {
        object.diagnostics.push(("schema_root_unavailable").into());
        return;
    };
    object.root = Some(PageIdentity { page_number: root });
    for number in trees.pages(root) {
        if let Err(code) = check_stop(control) {
            object.state = SchemaState::Partial;
            object.diagnostics.push((code).into());
            return;
        }
        attributions.insert((number, position), ());
        if let Err(code) = check_stop(control) {
            object.state = SchemaState::Partial;
            object.diagnostics.push(code.into());
            return;
        }
        object.attributed_through = Some(number);
    }
    object.state = if partial {
        SchemaState::Partial
    } else {
        SchemaState::Complete
    };
    if partial {
        object
            .diagnostics
            .push(("schema_attribution_partial").into());
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
