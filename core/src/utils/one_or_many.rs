//! OneOrMany utility type for handling single or multiple values
//!
//! This replaces `rig::OneOrMany` to remove the rig-core dependency.

use serde::{Deserialize, Serialize};

/// A type that can hold either one value or many values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    /// Single value
    One(T),
    /// Multiple values
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    /// Create from a single value
    pub fn one(value: T) -> Self {
        Self::One(value)
    }

    /// Create from multiple values
    pub fn many(values: Vec<T>) -> Self {
        Self::Many(values)
    }

    /// Iterate over the contained value(s)
    pub fn iter(&self) -> OneOrManyIter<'_, T> {
        match self {
            Self::One(v) => OneOrManyIter::One(std::iter::once(v)),
            Self::Many(vs) => OneOrManyIter::Many(vs.iter()),
        }
    }

    /// Get the number of elements
    pub fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(vs) => vs.len(),
        }
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Many(vs) if vs.is_empty())
    }

    /// Get the first element, if any
    pub fn first(&self) -> Option<&T> {
        match self {
            Self::One(v) => Some(v),
            Self::Many(vs) => vs.first(),
        }
    }
}

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl<T> From<T> for OneOrMany<T> {
    fn from(value: T) -> Self {
        Self::One(value)
    }
}

impl<T> From<Vec<T>> for OneOrMany<T> {
    fn from(values: Vec<T>) -> Self {
        Self::Many(values)
    }
}

/// Iterator for OneOrMany
pub enum OneOrManyIter<'a, T> {
    One(std::iter::Once<&'a T>),
    Many(std::slice::Iter<'a, T>),
}

impl<'a, T> Iterator for OneOrManyIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::One(iter) => iter.next(),
            Self::Many(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::One(iter) => iter.size_hint(),
            Self::Many(iter) => iter.size_hint(),
        }
    }
}

impl<'a, T> ExactSizeIterator for OneOrManyIter<'a, T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_one_value() {
        let one: OneOrMany<i32> = OneOrMany::one(42);
        assert_eq!(one.len(), 1);
        assert!(!one.is_empty());
        assert_eq!(one.first(), Some(&42));

        let items: Vec<_> = one.iter().collect();
        assert_eq!(items, vec![&42]);
    }

    #[test]
    fn test_many_values() {
        let many: OneOrMany<i32> = OneOrMany::many(vec![1, 2, 3]);
        assert_eq!(many.len(), 3);
        assert!(!many.is_empty());
        assert_eq!(many.first(), Some(&1));

        let items: Vec<_> = many.iter().collect();
        assert_eq!(items, vec![&1, &2, &3]);
    }

    #[test]
    fn test_empty_many() {
        let empty: OneOrMany<i32> = OneOrMany::many(vec![]);
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
        assert_eq!(empty.first(), None);
    }

    #[test]
    fn test_from_single() {
        let one: OneOrMany<&str> = "hello".into();
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn test_from_vec() {
        let many: OneOrMany<&str> = vec!["a", "b"].into();
        assert_eq!(many.len(), 2);
    }
}
