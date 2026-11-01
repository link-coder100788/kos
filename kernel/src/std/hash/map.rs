use alloc::{vec::Vec};
use crate::std::hash::fnv::fnv1a_64;
use core::{borrow::Borrow, ops::Index};

#[derive(Debug, Clone)]
pub enum Bucket<K, V> {
    Empty,
    Tombstone,
    Occupied { hash: u64, key: K, value: V },
}

pub struct HashMap<K, V> {
    pub buckets: Vec<Bucket<K, V>>,
    pub size: usize,
}

impl<K, V> HashMap<K, V>
where K: PartialEq + AsRef<[u8]>
{
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        let mut buckets = Vec::with_capacity(cap);
        buckets.resize_with(cap, || Bucket::Empty);
        Self { buckets, size: 0 }
    }

    fn calculate_hash(&self, key: &K) -> u64 {
        let bytes = key.as_ref();
        fnv1a_64(bytes)
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.size * 4 >= self.buckets.len() * 3 {
            self.resize();
        }

        let hash = self.calculate_hash(&key);
        let mut idx = (hash as usize) & (self.buckets.len() - 1);
        let mut first_tombstone_idx: Option<usize> = None;
        loop {
            match &mut self.buckets[idx] {
                Bucket::Empty => {
                    if let Some(tomb_idx) = first_tombstone_idx {
                        self.buckets[tomb_idx] = Bucket::Occupied { hash, key, value };
                    } else {
                        self.buckets[idx] = Bucket::Occupied { hash, key, value };
                    }
                    self.size += 1;
                    return None;
                }
                Bucket::Tombstone => {
                    if first_tombstone_idx.is_none() {
                        first_tombstone_idx = Some(idx);
                    }
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
                Bucket::Occupied { hash: occupied_hash, key: existing_key, value: existing_value } => {
                    if *occupied_hash == hash && *existing_key == key {
                        return Some(core::mem::replace(existing_value, value));
                    }
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
            }
        }
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let bytes = key.as_ref();
        let hash = fnv1a_64(bytes);
        let mut idx = (hash as usize) & (self.buckets.len() - 1);
        loop {
            match &self.buckets[idx] {
                Bucket::Empty => return None,
                Bucket::Occupied { hash: occupied_hash, key: existing_key, value } => {
                    if *occupied_hash == hash && existing_key.borrow() == key {
                        return Some(value);
                    }
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
                Bucket::Tombstone => {
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
            }
        }
    }

    fn resize(&mut self) {
        let new_capacity = if self.buckets.is_empty() { 8 } else { self.buckets.len() * 2 };
        let old_buckets = core::mem::replace(&mut self.buckets, {
            let mut new_vec = Vec::with_capacity(new_capacity);
            new_vec.resize_with(new_capacity, || Bucket::Empty);
            new_vec
        });
        self.size = 0;
        for bucket in old_buckets {
            if let Bucket::Occupied { key, value, .. } = bucket {
                self.insert(key, value);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub fn get_and_then<Q, F, T>(&self, key: &Q, f: F) -> Option<T>
    where for<'a> F: FnOnce(&'a V) -> Option<T>,
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let bytes = key.as_ref();
        let hash = fnv1a_64(bytes);
        let mut idx = (hash as usize) & (self.buckets.len() - 1);
        loop {
            match &self.buckets[idx] {
                Bucket::Empty => return None,
                Bucket::Occupied { hash: occupied_hash, key: existing_key, value } => {
                    if *occupied_hash == hash && existing_key.borrow() == key {
                        return f(&value);
                    }
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
                Bucket::Tombstone => {
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
            }
        }
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter { inner: self.buckets.iter() }
    }

    pub fn prepare_remove<'a, Q>(&'a mut self, key: &Q) -> Option<impl FnOnce() -> V + 'a>
    where K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let bytes = key.as_ref();
        let hash = fnv1a_64(bytes);
        let mut idx = (hash as usize) & (self.buckets.len() - 1);
        let target_idx = loop {
            match &self.buckets[idx] {
                Bucket::Empty => return None,
                Bucket::Tombstone => {
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
                Bucket::Occupied { hash: occupied_hash, key: existing_key, .. } => {
                    if *occupied_hash == hash && existing_key.borrow() == key {
                        break idx;
                    }
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
            }
        };
        Some(move || {
            let old_bucket = core::mem::replace(&mut self.buckets[target_idx], Bucket::Tombstone);
            if let Bucket::Occupied { value, .. } = old_bucket {
                self.size -= 1;
                value
            } else {
                unreachable!("Bucket state corrupted during deferred removal")
            }
        })
    }

    pub fn get_and_then_delete<'a, Q, F, T>(&'a mut self, key: &Q, f: F) -> (Option<impl FnOnce() -> V + 'a>, Option<T>)
    where K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
        for<'b> F: FnOnce(&'b V) -> Option<T>,
    {
        if self.buckets.is_empty() {
            return (None, None);
        }
        let bytes = key.as_ref();
        let hash = fnv1a_64(bytes);
        let mut idx = (hash as usize) & (self.buckets.len() - 1);
        let mut result = None;
        let target_idx = loop {
            match &self.buckets[idx] {
                Bucket::Empty => return (None, None),
                Bucket::Tombstone => {
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
                Bucket::Occupied { hash: occupied_hash, key: existing_key, value } => {
                    if *occupied_hash == hash && existing_key.borrow() == key {
                        result = f(&value);
                        break idx;
                    }
                    idx = (idx + 1) & (self.buckets.len() - 1);
                }
            }
        };
        (Some(move || {
            let old_bucket = core::mem::replace(&mut self.buckets[target_idx], Bucket::Tombstone);
            if let Bucket::Occupied { value, .. } = old_bucket {
                self.size -= 1;
                value
            } else {
                unreachable!("Bucket state corrupted during deferred removal")
            }
        }), result)
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        self.get(key).is_some()
    }

    pub fn values(&self) -> Values<'_, K, V> {
        Values { inner: self.buckets.iter() }
    }
}

pub struct Iter<'a, K, V> {
    inner: core::slice::Iter<'a, Bucket<K, V>>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(bucket) = self.inner.next() {
            if let Bucket::Occupied { key, value, .. } = bucket {
                return Some((key, value));
            }
        }
        None
    }
}

pub struct Values<'a, K, V> {
    inner: core::slice::Iter<'a, Bucket<K, V>>,
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(bucket) = self.inner.next() {
            if let Bucket::Occupied { value, .. } = bucket {
                return Some(value);
            }
        }
        None
    }
}

impl<K, V, Q> Index<&Q> for HashMap<K, V> 
where K: Borrow<Q> + AsRef<[u8]> + PartialEq,
    Q: PartialEq + AsRef<[u8]> + ?Sized,
{
    type Output = V;

    #[inline]
    fn index(&self, index: &Q) -> &Self::Output {
        self.get(index).expect("key not found")
    }
}