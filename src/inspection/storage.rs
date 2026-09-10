//! Private structural inventory storage. No application payload bytes are retained.
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};
use serde::Serialize;
use tempfile::NamedTempFile;

use super::PageEntity;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageBudget {
    /// Estimated decoded inventory bytes retained before spilling.
    pub cache_bytes: u64,
    /// Maximum private `SQLite` file size; zero disables spilling.
    pub max_spill_bytes: u64,
}

impl Default for StorageBudget {
    fn default() -> Self {
        Self {
            cache_bytes: 8 * 1024 * 1024,
            max_spill_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub spilled: bool,
    pub spilled_indexes: u64,
    pub cache_bytes: u64,
    pub spill_bytes: u64,
    pub stored_pages: usize,
    pub inventory_bytes: u64,
    pub max_page_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum StorageError {
    Budget,
    Unavailable,
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull) {
            Self::Budget
        } else {
            Self::Unavailable
        }
    }
}

pub(super) struct DiskPages {
    pub(super) connection: Mutex<Connection>,
    cache: Mutex<PageCache>,
    // Keep the private 0600 file alive until all readers release the store.
    file: NamedTempFile,
}

struct PageCache {
    limit: u64,
    bytes: u64,
    clock: u64,
    entries: std::collections::BTreeMap<u32, (PageEntity, u64, u64)>,
}

impl PageCache {
    fn get(&mut self, number: u32) -> Option<PageEntity> {
        self.clock = self.clock.wrapping_add(1);
        let (page, _, used) = self.entries.get_mut(&number)?;
        *used = self.clock;
        Some(page.clone())
    }

    fn remove(&mut self, number: u32) {
        if let Some((_, size, _)) = self.entries.remove(&number) {
            self.bytes -= size;
        }
    }

    fn insert(&mut self, page: &PageEntity, size: u64) {
        self.remove(page.number);
        if size > self.limit {
            return;
        }
        while self.bytes.saturating_add(size) > self.limit {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, used))| used)
                .map(|(number, _)| *number)
            else {
                break;
            };
            self.remove(oldest);
        }
        self.clock = self.clock.wrapping_add(1);
        self.entries
            .insert(page.number, (page.clone(), size, self.clock));
        self.bytes += size;
    }
}

impl DiskPages {
    fn new(limit: u64, cache_bytes: u64) -> Result<Self, StorageError> {
        if limit < 28672 {
            return Err(StorageError::Budget);
        }
        let file = NamedTempFile::new().map_err(|_| StorageError::Unavailable)?;
        let connection = Connection::open(file.path())?;
        connection.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=DELETE; PRAGMA synchronous=OFF; PRAGMA mmap_size=0; PRAGMA cache_size=-64; PRAGMA temp_store=FILE;")?;
        connection.pragma_update(None, "max_page_count", limit / 4096)?;
        connection.execute_batch(
            "CREATE TABLE pages(number INTEGER PRIMARY KEY, evidence BLOB NOT NULL);
             CREATE TABLE records(collection INTEGER NOT NULL, position INTEGER NOT NULL, evidence BLOB NOT NULL, source_page INTEGER, target_page INTEGER, PRIMARY KEY(collection,position));
             CREATE INDEX record_sources ON records(collection,source_page,position) WHERE source_page IS NOT NULL;
             CREATE INDEX record_targets ON records(collection,target_page,position) WHERE target_page IS NOT NULL;
             CREATE TABLE map_values(collection INTEGER NOT NULL, key BLOB NOT NULL, evidence BLOB NOT NULL, PRIMARY KEY(collection,key)) WITHOUT ROWID;",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            cache: Mutex::new(PageCache {
                limit: cache_bytes,
                bytes: 0,
                clock: 0,
                entries: std::collections::BTreeMap::new(),
            }),
            file,
        })
    }

    fn insert(&self, number: u32, bytes: &[u8]) -> Result<(), StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .execute(
                "INSERT OR REPLACE INTO pages VALUES(?1, ?2)",
                params![number, bytes],
            )?;
        self.cache
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .remove(number);
        Ok(())
    }

    fn get(&self, number: u32) -> Result<PageEntity, StorageError> {
        if let Some(page) = self
            .cache
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .get(number)
        {
            return Ok(page);
        }
        let bytes: Vec<u8> = self
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .query_row(
                "SELECT evidence FROM pages WHERE number=?1",
                [number],
                |row| row.get(0),
            )?;
        let page = serde_json::from_slice(&bytes).map_err(|_| StorageError::Unavailable)?;
        self.cache
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .insert(&page, bytes.len() as u64 * 4);
        Ok(page)
    }

    fn encoded_size(&self, number: u32) -> Result<u64, StorageError> {
        Ok(self
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .query_row(
                "SELECT length(evidence) FROM pages WHERE number=?1",
                [number],
                |row| row.get(0),
            )?)
    }

    fn bytes(&self) -> u64 {
        self.file.as_file().metadata().map_or(0, |m| m.len())
    }
}

pub(super) struct StoreContext {
    pub budget: StorageBudget,
    disk: Mutex<Option<Arc<DiskPages>>>,
    failure: std::sync::atomic::AtomicU8,
    pub index_bytes: Mutex<u64>,
    pub next_collection: std::sync::atomic::AtomicU64,
    pub spilled_indexes: std::sync::atomic::AtomicU64,
}

impl StoreContext {
    pub(super) fn new(budget: StorageBudget) -> Self {
        Self {
            budget,
            disk: Mutex::new(None),
            failure: std::sync::atomic::AtomicU8::new(0),
            index_bytes: Mutex::new(0),
            spilled_indexes: std::sync::atomic::AtomicU64::new(0),
            next_collection: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub(super) fn disk(&self) -> Result<Arc<DiskPages>, StorageError> {
        let mut slot = self.disk.lock().map_err(|_| StorageError::Unavailable)?;
        if let Some(disk) = &*slot {
            return Ok(Arc::clone(disk));
        }
        let disk = Arc::new(DiskPages::new(
            self.budget.max_spill_bytes,
            self.budget.cache_bytes / 2,
        )?);
        *slot = Some(Arc::clone(&disk));
        Ok(disk)
    }

    pub(super) fn check(&self) -> Result<(), StorageError> {
        match self.failure.load(Ordering::Acquire) {
            0 => Ok(()),
            1 => Err(StorageError::Budget),
            _ => Err(StorageError::Unavailable),
        }
    }

    pub(super) fn record_error(&self, error: StorageError) {
        let value = match error {
            StorageError::Budget => 1,
            StorageError::Unavailable => 2,
        };
        self.failure.fetch_max(value, Ordering::AcqRel);
    }

    pub(super) fn clear_budget_stop(&self) {
        let _ = self
            .failure
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(super) fn reserve(&self, before: u64, after: u64) -> bool {
        let mut bytes = self
            .index_bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = bytes.saturating_sub(before).saturating_add(after);
        if next > self.budget.cache_bytes / 2 {
            return false;
        }
        *bytes = next;
        true
    }

    pub(super) fn release(&self, count: u64) {
        let mut bytes = self
            .index_bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *bytes = bytes.saturating_sub(count);
    }
}

#[derive(Clone)]
struct Pages {
    memory: Arc<Vec<PageEntity>>,
    disk: Option<Arc<DiskPages>>,
}

#[derive(Clone)]
pub(super) struct PageStore {
    budget: StorageBudget,
    pages: Pages,
    status: StorageStatus,
    pub(super) context: Arc<StoreContext>,
    classifications: Option<Arc<super::index::Sequence<super::PageClassification>>>,
    allocation: Arc<super::index_map::IndexMap<u32, PageEntity>>,
    pub(super) schema: Option<Arc<super::schema::StoredSchema>>,
    pub(super) pointer_maps: Arc<super::index::Sequence<super::PointerMapPage>>,
    pub(super) freelist_trunks: Arc<super::index::Sequence<super::FreelistTrunk>>,
    pub(super) diagnostics: Arc<super::index::Sequence<super::StructuralDiagnostic>>,
    pub(super) claims: Arc<super::index::Sequence<super::RelationshipClaim>>,
    pub(super) relationships: Arc<super::index::Sequence<super::Relationship>>,
    pub(super) traversals: Arc<super::index::Sequence<super::Traversal>>,
}

impl PageStore {
    pub(super) fn new(budget: StorageBudget) -> Self {
        Self {
            budget,
            pages: Pages {
                memory: Arc::new(Vec::new()),
                disk: None,
            },
            status: StorageStatus::default(),
            context: Arc::new(StoreContext::new(budget)),
            classifications: None,
            allocation: Arc::new(super::index_map::IndexMap::default()),
            schema: None,
            pointer_maps: Arc::new(super::index::Sequence::default()),
            freelist_trunks: Arc::new(super::index::Sequence::default()),
            diagnostics: Arc::new(super::index::Sequence::default()),
            claims: Arc::new(super::index::Sequence::default()),
            relationships: Arc::new(super::index::Sequence::default()),
            traversals: Arc::new(super::index::Sequence::default()),
        }
    }

    pub(super) fn status(&self) -> StorageStatus {
        let mut status = self.status.clone();
        if let Some(disk) = &self.pages.disk {
            status.cache_bytes += disk.cache.lock().map_or(0, |cache| cache.bytes);
        }
        if let Ok(slot) = self.context.disk.lock()
            && let Some(disk) = &*slot
        {
            status.spilled = true;
            status.spill_bytes = disk.bytes();
        }
        status.spilled_indexes = self.context.spilled_indexes.load(Ordering::Relaxed);
        status.cache_bytes += self.context.index_bytes.lock().map_or(0, |n| *n);
        status
    }

    pub(super) fn len(&self) -> usize {
        self.status.stored_pages
    }

    pub(super) fn export_reservation(&self) -> Result<u64, StorageError> {
        let classifications = self
            .classifications
            .as_ref()
            .map(|values| values.estimated_total())
            .transpose()?
            .unwrap_or(0);
        Ok(self
            .status
            .inventory_bytes
            .saturating_add(classifications)
            .saturating_add(self.allocation.estimated_total()?)
            .saturating_add(
                self.schema
                    .as_ref()
                    .map(|schema| schema.estimated_total())
                    .transpose()?
                    .unwrap_or(0),
            )
            .saturating_add(self.pointer_maps.estimated_total()?)
            .saturating_add(self.pointer_maps.len() as u64 * 128)
            .saturating_add(self.freelist_trunks.estimated_total()?)
            .saturating_add(self.diagnostics.estimated_total()?)
            .saturating_add(self.claims.estimated_total()?)
            .saturating_add(self.relationships.estimated_total()?)
            .saturating_add(self.traversals.estimated_total()?))
    }

    pub(super) fn estimated_size(&self, index: usize) -> Result<u64, StorageError> {
        let raw = if let Some(page) = self.pages.memory.get(index) {
            super::budget::serialized_reservation(page).ok_or(StorageError::Unavailable)?
        } else {
            self.pages
                .disk
                .as_ref()
                .ok_or(StorageError::Unavailable)?
                .encoded_size(u32::try_from(index + 1).map_err(|_| StorageError::Unavailable)?)?
                .saturating_mul(4)
        };
        let number = u32::try_from(index + 1).map_err(|_| StorageError::Unavailable)?;
        let allocation = self.allocation.estimated_size(&number)?;
        let classification = self
            .classifications
            .as_ref()
            .map(|values| values.estimated_size(index))
            .transpose()?
            .unwrap_or(0);
        Ok(raw.max(allocation).saturating_add(classification))
    }

    /// An I/O failure poisons this inventory and all its views. Publication must
    /// call `check` so missing private evidence cannot become a source diagnostic.
    pub(super) fn get_admitted(
        &self,
        index: usize,
        ceiling: u64,
    ) -> Result<Option<PageEntity>, StorageError> {
        if index >= self.len() {
            return Ok(None);
        }
        let reservation = self.estimated_size(index)?.saturating_mul(2);
        if !super::budget::memory_available(ceiling, reservation) {
            return Err(StorageError::Budget);
        }
        let page = self.get(index);
        self.check()?;
        Ok(page)
    }

    pub(super) fn get(&self, index: usize) -> Option<PageEntity> {
        if index >= self.len() || matches!(self.context.check(), Err(StorageError::Unavailable)) {
            return None;
        }
        let mut page = if let Some(page) = self.allocation.get(&u32::try_from(index + 1).ok()?) {
            page
        } else if let Some(page) = self.pages.memory.get(index) {
            page.clone()
        } else if let Some(disk) = &self.pages.disk {
            if let Ok(page) = disk.get(u32::try_from(index + 1).ok()?) {
                page
            } else {
                self.context.record_error(StorageError::Unavailable);
                return None;
            }
        } else {
            return None;
        };
        if let Some(classifications) = &self.classifications
            && let Some(classification) = classifications.get(index)
        {
            page.classification = classification;
        }
        Some(page)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = PageEntity> + '_ {
        (0..self.len()).map_while(|index| self.get(index))
    }

    pub(super) fn check(&self) -> Result<(), StorageError> {
        self.context.check()
    }

    pub(super) fn push(&mut self, page: PageEntity) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec(&page).map_err(|_| StorageError::Unavailable)?;
        // JSON is compact relative to the nested decoded vectors and strings.
        let retained = (bytes.len() as u64).saturating_mul(4);
        if self.pages.disk.is_none() {
            if self.status.cache_bytes.saturating_add(retained) <= self.budget.cache_bytes / 2 {
                Arc::make_mut(&mut self.pages.memory).push(page);
                self.status.cache_bytes += retained;
                self.status.stored_pages += 1;
                self.status.inventory_bytes += retained;
                self.status.max_page_bytes = self.status.max_page_bytes.max(retained);
                return Ok(());
            }
            let disk = self.context.disk()?;
            {
                let mut cache = disk.cache.lock().map_err(|_| StorageError::Unavailable)?;
                cache.limit = (self.budget.cache_bytes / 2).saturating_sub(self.status.cache_bytes);
                cache.entries.clear();
                cache.bytes = 0;
            }
            self.pages.disk = Some(disk);
            self.status.spilled = true;
        }
        if let Some(disk) = &self.pages.disk {
            disk.insert(page.number, &bytes)?;
            self.status.spill_bytes = disk.bytes();
            self.status.stored_pages += 1;
            self.status.inventory_bytes += retained;
            self.status.max_page_bytes = self.status.max_page_bytes.max(retained);
        }
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        classifications: super::index::Sequence<super::PageClassification>,
        allocation: super::index_map::IndexMap<u32, PageEntity>,
    ) {
        self.classifications = Some(Arc::new(classifications));
        self.allocation = Arc::new(allocation);
    }

    pub(super) fn materialize(&self) -> Result<Vec<PageEntity>, StorageError> {
        let pages = self.iter().collect();
        self.check()?;
        Ok(pages)
    }
}
