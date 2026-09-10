//! Work sequences keep a bounded cache and spill individual records to a shared file.
//! Crossing the cache ceiling never starts a bulk migration between work checkpoints.
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rusqlite::params;
use serde::{Serialize, de::DeserializeOwned};

use super::storage::{StorageError, StoreContext};

/// Private encoding can retain validation state omitted from public JSON.
pub(super) trait StoredRecord: Clone + Serialize + DeserializeOwned {
    fn page_tags(&self) -> (Option<u32>, Option<u32>) {
        (None, None)
    }
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
    fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}
impl StoredRecord for u32 {}
impl StoredRecord for bool {}
impl<T: StoredRecord> StoredRecord for Vec<T> {}
impl StoredRecord for super::PageClassification {}
impl StoredRecord for super::Relationship {
    fn page_tags(&self) -> (Option<u32>, Option<u32>) {
        (
            Some(entity_page(&self.source)),
            Some(self.target.page_number),
        )
    }
}
impl StoredRecord for super::Traversal {
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        traversals::encode(self)
    }
    fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        traversals::decode(bytes)
    }
    fn page_tags(&self) -> (Option<u32>, Option<u32>) {
        (Some(entity_page(&self.origin)), None)
    }
}
impl StoredRecord for super::FreelistTrunk {
    fn page_tags(&self) -> (Option<u32>, Option<u32>) {
        (Some(self.page.page_number), None)
    }
}
impl StoredRecord for super::PointerMapPage {
    fn page_tags(&self) -> (Option<u32>, Option<u32>) {
        (Some(self.page.page_number), None)
    }
}
impl StoredRecord for super::StructuralDiagnostic {}
impl StoredRecord for super::SchemaObject {
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&(self, self.attributed_through))
    }
    fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let (mut object, through): (Self, Option<u32>) = serde_json::from_slice(bytes)?;
        object.attributed_through = through;
        Ok(object)
    }
}
impl StoredRecord for super::RelationshipClaim {
    fn page_tags(&self) -> (Option<u32>, Option<u32>) {
        (
            Some(entity_page(&self.source)),
            self.target.as_ref().map(|page| page.page_number),
        )
    }
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&(self, self.stop_reason))
    }
    fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let (mut claim, reason): (Self, Option<super::TraversalStopReason>) =
            serde_json::from_slice(bytes)?;
        claim.stop_reason = reason;
        Ok(claim)
    }
}

fn entity_page(entity: &super::EntityIdentity) -> u32 {
    match entity {
        super::EntityIdentity::Page { page_number }
        | super::EntityIdentity::Cell { page_number, .. } => *page_number,
    }
}

mod pages;
mod traversals;

pub(super) struct Sequence<T: StoredRecord> {
    context: Arc<StoreContext>,
    collection: u64,
    memory: std::collections::BTreeMap<usize, T>,
    spilled: bool,
    bytes: u64,
    length: usize,
    segments: Vec<Self>,
}

impl<T: StoredRecord> Sequence<T> {
    pub(super) fn new(context: Arc<StoreContext>) -> Self {
        let collection = context.next_collection.fetch_add(1, Ordering::Relaxed);
        Self {
            context,
            collection,
            memory: std::collections::BTreeMap::new(),
            spilled: false,
            bytes: 0,
            length: 0,
            segments: Vec::new(),
        }
    }

    pub(super) fn sibling<U: StoredRecord>(&self) -> Sequence<U> {
        Sequence::new(Arc::clone(&self.context))
    }

    pub(super) fn map<K: super::index_map::Key, V: Clone + Serialize + DeserializeOwned>(
        &self,
    ) -> super::index_map::IndexMap<K, V> {
        super::index_map::IndexMap::new(Arc::clone(&self.context))
    }

    pub(super) fn clear(&mut self) {
        *self = Self::new(Arc::clone(&self.context));
    }

    pub(super) fn append(&mut self, other: &mut Self) {
        if other.len() > 0 {
            let empty = Self::new(Arc::clone(&other.context));
            self.segments.push(std::mem::replace(other, empty));
        }
    }

    pub(super) fn retain(
        &mut self,
        mut keep: impl FnMut(&T) -> bool,
        mut proceed: impl FnMut() -> bool,
    ) {
        let mut retained = Self::new(Arc::clone(&self.context));
        for value in self.iter() {
            if !proceed() {
                break;
            }
            if keep(&value) {
                retained.push(value);
            }
        }
        *self = retained;
    }

    pub(super) fn materialize(&self) -> Result<Vec<T>, StorageError> {
        let values = self.iter().collect();
        self.context.check()?;
        Ok(values)
    }

    pub(super) fn len(&self) -> usize {
        self.length + self.segments.iter().map(Self::len).sum::<usize>()
    }

    fn write(
        &self,
        position: usize,
        bytes: &[u8],
        tags: (Option<u32>, Option<u32>),
    ) -> Result<(), StorageError> {
        self.context
            .disk()?
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .execute(
                "INSERT OR REPLACE INTO records VALUES(?1,?2,?3,?4,?5)",
                params![self.collection, position, bytes, tags.0, tags.1],
            )?;
        Ok(())
    }

    pub(super) fn push(&mut self, value: T) {
        if self.context.check().is_err() {
            return;
        }
        if let Err(error) = self.store(self.len(), value) {
            self.context.record_error(error);
        }
    }

    fn store(&mut self, position: usize, value: T) -> Result<(), StorageError> {
        if position >= self.length && !self.segments.is_empty() {
            let mut remaining = position - self.length;
            let last = self.segments.len() - 1;
            for (index, segment) in self.segments.iter_mut().enumerate() {
                if remaining < segment.len() || index == last {
                    return segment.store(remaining, value);
                }
                remaining -= segment.len();
            }
            return Err(StorageError::Unavailable);
        }
        let encoded = value.encode().map_err(|_| StorageError::Unavailable)?;
        let size = (encoded.len() as u64 + 64) * 4;
        let append = position == self.length;
        let old = self
            .memory
            .get(&position)
            .map(|prior| prior.encode().map(|bytes| (bytes.len() as u64 + 64) * 4))
            .transpose()
            .map_err(|_| StorageError::Unavailable)?
            .unwrap_or(0);
        if (!self.spilled || self.memory.contains_key(&position)) && self.context.reserve(old, size)
        {
            self.memory.insert(position, value);
            self.bytes = self.bytes.saturating_sub(old).saturating_add(size);
            if append {
                self.length += 1;
            }
            return Ok(());
        }
        // Persist only this record. Cached records remain readable in place; a changed
        // cached record is removed only after its replacement is durable.
        self.write(position, &encoded, value.page_tags())?;
        if !self.spilled {
            self.spilled = true;
            self.context.spilled_indexes.fetch_add(1, Ordering::Relaxed);
        }
        if self.memory.remove(&position).is_some() {
            self.context.release(old);
            self.bytes = self.bytes.saturating_sub(old);
        }
        if append {
            self.length += 1;
        }
        Ok(())
    }

    pub(super) fn estimated_total(&self) -> Result<u64, StorageError> {
        self.segments
            .iter()
            .try_fold(self.local_estimated_total()?, |total, segment| {
                Ok(total.saturating_add(segment.estimated_total()?))
            })
    }

    fn local_estimated_total(&self) -> Result<u64, StorageError> {
        if self.length == 0 {
            return Ok(0);
        }
        if !self.spilled {
            return Ok(self.bytes);
        }
        let length: u64 = self
            .context
            .disk()?
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .query_row(
                "SELECT coalesce(sum(length(evidence)),0) FROM records WHERE collection=?1",
                params![self.collection],
                |row| row.get(0),
            )?;
        Ok(self.bytes.saturating_add(length.saturating_mul(4)))
    }

    pub(super) fn estimated_size(&self, position: usize) -> Result<u64, StorageError> {
        if position >= self.length {
            let mut remaining = position - self.length;
            for segment in &self.segments {
                if remaining < segment.len() {
                    return segment.estimated_size(remaining);
                }
                remaining -= segment.len();
            }
            return Ok(0);
        }
        if let Some(value) = self.memory.get(&position) {
            return super::budget::serialized_reservation(value)
                .map(|bytes| bytes.saturating_add(256))
                .ok_or(StorageError::Unavailable);
        }
        let length: u64 = self
            .context
            .disk()?
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .query_row(
                "SELECT length(evidence) FROM records WHERE collection=?1 AND position=?2",
                params![self.collection, position],
                |row| row.get(0),
            )?;
        Ok(length.saturating_mul(4))
    }

    pub(super) fn get(&self, position: usize) -> Option<T> {
        if self.context.check().is_err() {
            return None;
        }
        if position >= self.length {
            let mut remaining = position - self.length;
            for segment in &self.segments {
                if remaining < segment.len() {
                    return segment.get(remaining);
                }
                remaining -= segment.len();
            }
            return None;
        }
        if let Some(value) = self.memory.get(&position) {
            return Some(value.clone());
        }
        if !self.spilled {
            return None;
        }
        let read = || -> Result<T, StorageError> {
            let bytes: Vec<u8> = self
                .context
                .disk()?
                .connection
                .lock()
                .map_err(|_| StorageError::Unavailable)?
                .query_row(
                    "SELECT evidence FROM records WHERE collection=?1 AND position=?2",
                    params![self.collection, position],
                    |row| row.get(0),
                )?;
            T::decode(&bytes).map_err(|_| StorageError::Unavailable)
        };
        match read() {
            Ok(value) => Some(value),
            Err(error) => {
                self.context.record_error(error);
                None
            }
        }
    }

    pub(super) fn edit(&mut self, position: usize, change: impl FnOnce(&mut T)) -> bool {
        let Some(mut value) = self.get(position) else {
            return false;
        };
        change(&mut value);
        match self.store(position, value) {
            Ok(()) => true,
            Err(error) => {
                self.context.record_error(error);
                false
            }
        }
    }

    pub(super) fn last_mut(&mut self) -> Option<RecordMut<'_, T>> {
        self.get_mut(self.len().checked_sub(1)?)
    }

    pub(super) fn get_mut(&mut self, position: usize) -> Option<RecordMut<'_, T>> {
        let value = self.get(position)?;
        Some(RecordMut {
            sequence: self,
            position,
            value: Some(value),
        })
    }

    pub(super) fn admit_record(&self, position: usize, admit: impl FnOnce(u64) -> bool) -> bool {
        match self.estimated_size(position) {
            Ok(bytes) => bytes <= 64 * 1024 || admit(bytes),
            Err(error) => {
                self.context.record_error(error);
                false
            }
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = T> + '_ {
        (0..self.len()).map_while(|position| self.get(position))
    }
}

impl<T: StoredRecord> Drop for Sequence<T> {
    fn drop(&mut self) {
        self.context.release(self.bytes);
    }
}

pub(super) struct RecordMut<'a, T: StoredRecord> {
    sequence: &'a mut Sequence<T>,
    position: usize,
    value: Option<T>,
}

impl<T: StoredRecord> Deref for RecordMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value.as_ref().expect("live record guard")
    }
}

impl<T: StoredRecord> DerefMut for RecordMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value.as_mut().expect("live record guard")
    }
}

impl<T: StoredRecord> Drop for RecordMut<'_, T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take()
            && let Err(error) = self.sequence.store(self.position, value)
        {
            self.sequence.context.record_error(error);
        }
    }
}

pub(super) struct IntoIter<T: StoredRecord> {
    sequence: Sequence<T>,
    position: usize,
}

impl<T: StoredRecord> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        let value = self.sequence.get(self.position)?;
        self.position += 1;
        Some(value)
    }
}

impl<T: StoredRecord> IntoIterator for Sequence<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            sequence: self,
            position: 0,
        }
    }
}

impl<T: StoredRecord> Default for Sequence<T> {
    fn default() -> Self {
        Self::new(Arc::new(StoreContext::new(super::StorageBudget::default())))
    }
}

impl<'a, T: StoredRecord> IntoIterator for &'a Sequence<T> {
    type Item = T;
    type IntoIter =
        std::iter::MapWhile<std::ops::Range<usize>, Box<dyn FnMut(usize) -> Option<T> + 'a>>;
    fn into_iter(self) -> Self::IntoIter {
        (0..self.len()).map_while(Box::new(move |position| self.get(position)))
    }
}
