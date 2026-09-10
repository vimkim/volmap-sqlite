//! Ordered work indexes, sharing the structural store's cache and spill ceilings.
//! Cached and disk keys remain disjoint; updates persist before evicting a cached value.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use super::storage::{StorageError, StoreContext};

pub(super) trait Key: Ord + Clone {
    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Option<Self>;
}

impl Key for u32 {
    fn encode(&self) -> Vec<u8> {
        self.to_be_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        Some(Self::from_be_bytes(bytes.try_into().ok()?))
    }
}

impl Key for usize {
    fn encode(&self) -> Vec<u8> {
        (*self as u64).to_be_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        Self::try_from(u64::from_be_bytes(bytes.try_into().ok()?)).ok()
    }
}

impl Key for String {
    fn encode(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        Self::from_utf8(bytes.to_vec()).ok()
    }
}

impl Key for (u32, u32) {
    fn encode(&self) -> Vec<u8> {
        [self.0.to_be_bytes(), self.1.to_be_bytes()].concat()
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        Some((u32::decode(bytes.get(..4)?)?, u32::decode(bytes.get(4..)?)?))
    }
}

impl Key for (u32, usize) {
    fn encode(&self) -> Vec<u8> {
        [self.0.encode(), self.1.encode()].concat()
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        Some((
            u32::decode(bytes.get(..4)?)?,
            usize::decode(bytes.get(4..)?)?,
        ))
    }
}

pub(super) struct IndexMap<K: Key, V: Clone + Serialize + DeserializeOwned> {
    context: Arc<StoreContext>,
    collection: u64,
    memory: BTreeMap<K, V>,
    spilled: bool,
    bytes: u64,
}

impl<K: Key, V: Clone + Serialize + DeserializeOwned> IndexMap<K, V> {
    pub(super) fn new(context: Arc<StoreContext>) -> Self {
        let collection = context.next_collection.fetch_add(1, Ordering::Relaxed);
        Self {
            context,
            collection,
            memory: BTreeMap::new(),
            spilled: false,
            bytes: 0,
        }
    }

    fn record<T>(&self, result: Result<T, StorageError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.context.record_error(error);
                None
            }
        }
    }

    pub(super) fn check(&self) -> Result<(), StorageError> {
        self.context.check()
    }

    pub(super) fn estimated_total(&self) -> Result<u64, StorageError> {
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
                "SELECT coalesce(sum(length(evidence)),0) FROM map_values WHERE collection=?1",
                params![self.collection],
                |row| row.get(0),
            )?;
        Ok(self.bytes.saturating_add(length.saturating_mul(4)))
    }

    pub(super) fn estimated_size(&self, key: &K) -> Result<u64, StorageError> {
        if let Some(value) = self.memory.get(key) {
            return super::budget::serialized_reservation(value).ok_or(StorageError::Unavailable);
        }
        if !self.spilled {
            return Ok(0);
        }
        let length: Option<u64> = self
            .context
            .disk()?
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .query_row(
                "SELECT length(evidence) FROM map_values WHERE collection=?1 AND key=?2",
                params![self.collection, key.encode()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(length.unwrap_or(0).saturating_mul(4))
    }

    pub(super) fn get(&self, key: &K) -> Option<V> {
        if self.context.check().is_err() {
            return None;
        }
        if let Some(value) = self.memory.get(key) {
            return Some(value.clone());
        }
        if !self.spilled {
            return None;
        }
        let read = || -> Result<Option<V>, StorageError> {
            let bytes: Option<Vec<u8>> = self
                .context
                .disk()?
                .connection
                .lock()
                .map_err(|_| StorageError::Unavailable)?
                .query_row(
                    "SELECT evidence FROM map_values WHERE collection=?1 AND key=?2",
                    params![self.collection, key.encode()],
                    |row| row.get(0),
                )
                .optional()?;
            bytes
                .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| StorageError::Unavailable))
                .transpose()
        };
        self.record(read()).flatten()
    }

    /// Admit a potentially large value before cloning or deserializing it.
    pub(super) fn get_admitted(&self, key: &K, admit: impl FnOnce(u64) -> bool) -> Option<V> {
        let reservation = self.record(self.estimated_size(key))?;
        if reservation > 64 * 1024 && !admit(reservation) {
            return None;
        }
        self.get(key)
    }

    fn write(&self, key: &K, bytes: &[u8]) -> Result<(), StorageError> {
        self.context
            .disk()?
            .connection
            .lock()
            .map_err(|_| StorageError::Unavailable)?
            .execute(
                "INSERT OR REPLACE INTO map_values VALUES(?1,?2,?3)",
                params![self.collection, key.encode(), bytes],
            )?;
        Ok(())
    }

    pub(super) fn insert(&mut self, key: K, value: V) -> Option<V> {
        let previous = self.get(&key);
        if self.context.check().is_err() {
            return previous;
        }
        let result = self.store(key, value);
        self.record(result);
        previous
    }

    fn store(&mut self, key: K, value: V) -> Result<(), StorageError> {
        let encoded = serde_json::to_vec(&value).map_err(|_| StorageError::Unavailable)?;
        let size = (key.encode().len() as u64 + encoded.len() as u64 + 64) * 4;
        let before = self
            .memory
            .get(&key)
            .map(|prior| {
                serde_json::to_vec(prior)
                    .map(|bytes| (key.encode().len() as u64 + bytes.len() as u64 + 64) * 4)
            })
            .transpose()
            .map_err(|_| StorageError::Unavailable)?
            .unwrap_or(0);
        if (!self.spilled || self.memory.contains_key(&key)) && self.context.reserve(before, size) {
            self.memory.insert(key, value);
            self.bytes = self.bytes.saturating_sub(before).saturating_add(size);
            return Ok(());
        }
        self.write(&key, &encoded)?;
        if !self.spilled {
            self.spilled = true;
            self.context.spilled_indexes.fetch_add(1, Ordering::Relaxed);
        }
        if self.memory.remove(&key).is_some() {
            self.context.release(before);
            self.bytes = self.bytes.saturating_sub(before);
        }
        Ok(())
    }

    pub(super) fn remove(&mut self, key: &K) -> Option<V> {
        let value = self.get(key)?;
        if self.memory.contains_key(key) {
            let size =
                (key.encode().len() as u64 + serde_json::to_vec(&value).ok()?.len() as u64 + 64)
                    * 4;
            self.memory.remove(key);
            self.context.release(size);
            self.bytes = self.bytes.saturating_sub(size);
        } else {
            let remove = || -> Result<(), StorageError> {
                self.context
                    .disk()?
                    .connection
                    .lock()
                    .map_err(|_| StorageError::Unavailable)?
                    .execute(
                        "DELETE FROM map_values WHERE collection=?1 AND key=?2",
                        params![self.collection, key.encode()],
                    )?;
                Ok(())
            };
            self.record(remove())?;
        }
        Some(value)
    }

    pub(super) fn get_mut(&mut self, key: &K) -> Option<MapRecordMut<'_, K, V>> {
        let value = self.get(key)?;
        Some(MapRecordMut {
            map: self,
            key: key.clone(),
            value: Some(value),
        })
    }

    pub(super) fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = K> + '_ {
        self.keys_after(None)
    }

    pub(super) fn keys_after(&self, mut previous: Option<K>) -> impl Iterator<Item = K> + '_ {
        std::iter::from_fn(move || {
            if self.context.check().is_err() {
                return None;
            }
            let memory = match &previous {
                Some(key) => self
                    .memory
                    .range((std::ops::Bound::Excluded(key), std::ops::Bound::Unbounded))
                    .next(),
                None => self.memory.iter().next(),
            }
            .map(|(key, _)| key.clone());
            let disk = if self.spilled {
                let read = || -> Result<Option<K>, StorageError> {
                    let disk = self.context.disk()?;
                    let connection = disk
                        .connection
                        .lock()
                        .map_err(|_| StorageError::Unavailable)?;
                    let bytes: Option<Vec<u8>> = if let Some(key) = &previous {
                        connection.query_row("SELECT key FROM map_values WHERE collection=?1 AND key>?2 ORDER BY key LIMIT 1", params![self.collection, key.encode()], |row| row.get(0)).optional()?
                    } else {
                        connection.query_row("SELECT key FROM map_values WHERE collection=?1 ORDER BY key LIMIT 1", params![self.collection], |row| row.get(0)).optional()?
                    };
                    bytes
                        .map(|bytes| K::decode(&bytes).ok_or(StorageError::Unavailable))
                        .transpose()
                };
                self.record(read()).flatten()
            } else {
                None
            };
            let key = [memory, disk].into_iter().flatten().min()?;
            previous = Some(key.clone());
            Some(key)
        })
    }
}

impl<K: Key, V: Clone + Serialize + DeserializeOwned> Drop for IndexMap<K, V> {
    fn drop(&mut self) {
        self.context.release(self.bytes);
    }
}

pub(super) struct MapRecordMut<'a, K: Key, V: Clone + Serialize + DeserializeOwned> {
    map: &'a mut IndexMap<K, V>,
    key: K,
    value: Option<V>,
}
impl<K: Key, V: Clone + Serialize + DeserializeOwned> std::ops::Deref for MapRecordMut<'_, K, V> {
    type Target = V;
    fn deref(&self) -> &V {
        self.value.as_ref().expect("live map record")
    }
}
impl<K: Key, V: Clone + Serialize + DeserializeOwned> std::ops::DerefMut
    for MapRecordMut<'_, K, V>
{
    fn deref_mut(&mut self) -> &mut V {
        self.value.as_mut().expect("live map record")
    }
}
impl<K: Key, V: Clone + Serialize + DeserializeOwned> Drop for MapRecordMut<'_, K, V> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            let result = self.map.store(self.key.clone(), value);
            self.map.record(result);
        }
    }
}
impl<K: Key, V: Clone + Serialize + DeserializeOwned> Default for IndexMap<K, V> {
    fn default() -> Self {
        Self::new(Arc::new(StoreContext::new(super::StorageBudget::default())))
    }
}
