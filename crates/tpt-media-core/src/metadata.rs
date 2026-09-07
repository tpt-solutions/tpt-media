//! Free-form metadata: typed key/value pairs.

use alloc::string::String;
use alloc::vec::Vec;

/// Metadata value variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetadataValue {
    /// UTF-8 string.
    Text(String),
    /// Signed integer.
    Int(i64),
    /// Boolean.
    Bool(bool),
    /// Opaque bytes (e.g. cover art).
    Bytes(Vec<u8>),
}

impl MetadataValue {
    /// `true` if the value is empty (empty string, zero, false, empty bytes).
    pub fn is_empty(&self) -> bool {
        match self {
            MetadataValue::Text(s) => s.is_empty(),
            MetadataValue::Int(i) => *i == 0,
            MetadataValue::Bool(b) => !*b,
            MetadataValue::Bytes(b) => b.is_empty(),
        }
    }
}

/// Metadata key.
///
/// In-memory metadata uses strings as keys for ergonomics. The
/// serialization layer maps well-known keys to 4-byte IDs for the
/// compact on-disk format.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MetadataKey(pub String);

impl MetadataKey {
    /// Build a metadata key from a string.
    pub fn new(s: impl Into<String>) -> Self {
        MetadataKey(s.into())
    }
}

impl From<&str> for MetadataKey {
    fn from(s: &str) -> Self {
        MetadataKey(s.into())
    }
}

/// A metadata collection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    entries: Vec<(MetadataKey, MetadataValue)>,
}

impl Metadata {
    /// Empty metadata.
    pub fn new() -> Self {
        Metadata::default()
    }

    /// Insert a value.
    pub fn insert(&mut self, key: MetadataKey, value: MetadataValue) {
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = value;
        } else {
            self.entries.push((key, value));
        }
    }

    /// Look up a value by key.
    pub fn get(&self, key: &MetadataKey) -> Option<&MetadataValue> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Remove a value.
    pub fn remove(&mut self, key: &MetadataKey) -> Option<MetadataValue> {
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(pos).1)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over (key, value) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&MetadataKey, &MetadataValue)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

impl Extend<(MetadataKey, MetadataValue)> for Metadata {
    fn extend<T: IntoIterator<Item = (MetadataKey, MetadataValue)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let mut m = Metadata::new();
        m.insert(
            MetadataKey::new("title"),
            MetadataValue::Text("Hello".into()),
        );
        assert_eq!(
            m.get(&MetadataKey::new("title")),
            Some(&MetadataValue::Text("Hello".into()))
        );
    }

    #[test]
    fn insert_overwrites() {
        let mut m = Metadata::new();
        m.insert(MetadataKey::new("k"), MetadataValue::Int(1));
        m.insert(MetadataKey::new("k"), MetadataValue::Int(2));
        assert_eq!(m.get(&MetadataKey::new("k")), Some(&MetadataValue::Int(2)));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn remove() {
        let mut m = Metadata::new();
        m.insert(MetadataKey::new("k"), MetadataValue::Bool(true));
        assert_eq!(
            m.remove(&MetadataKey::new("k")),
            Some(MetadataValue::Bool(true))
        );
        assert!(m.is_empty());
    }
}
