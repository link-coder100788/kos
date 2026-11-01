use core::borrow::Borrow;

use crate::std::hash::map::HashMap;

pub struct HashSet<T> {
    map: HashMap<T, ()>,
}

impl<T> HashSet<T>
where T: PartialEq + AsRef<[u8]>,
{
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity),
        }
    }

    pub fn insert(&mut self, value: T) -> bool {
        self.map.insert(value, ()).is_none()
    }

    pub fn contains<Q>(&self, value: &Q) -> bool
    where T: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        self.map.get(value).is_some()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            inner: self.map.iter(),
        }
    }
}

pub struct Iter<'a, T> {
    inner: super::map::Iter<'a, T, ()>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(key, _value)| key)
    }
}