#![deny(rust_2018_idioms)]
#![feature(in_band_lifetimes)]
#![feature(box_patterns)]
#![feature(crate_visibility_modifier)]
#![feature(nll)]
#![feature(min_const_fn)]
#![feature(const_fn)]
#![feature(const_let)]
#![feature(try_from)]
#![feature(macro_at_most_once_rep)]
#![allow(dead_code)]
#![allow(unused_imports)]

use derive_new::new;
use rustc_hash::FxHashMap;
use std::any::Any;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Write;
use std::hash::Hash;

pub mod dyn_descriptor;
pub mod storage;

pub trait BaseQueryContext: Sized {
    /// A "query descriptor" packages up all the possible queries and a key.
    /// It is used to store information about (e.g.) the stack.
    ///
    /// At runtime, it can be implemented in various ways: a monster enum
    /// works for a fixed set of queries, but a boxed trait object is good
    /// for a more open-ended option.
    type QueryDescriptor: Debug + Eq;

    fn execute_query_implementation<Q>(
        &self,
        descriptor: Self::QueryDescriptor,
        key: &Q::Key,
    ) -> Q::Value
    where
        Q: Query<Self>;

    /// Reports an unexpected cycle attempting to access the query Q with the given key.
    fn report_unexpected_cycle(&self, descriptor: Self::QueryDescriptor) -> !;
}

pub trait Query<QC: BaseQueryContext>: Debug + Default + Sized + 'static {
    type Key: Clone + Debug + Hash + Eq;
    type Value: Clone + Debug + Hash + Eq;
    type Storage: QueryStorageOps<QC, Self>;

    fn execute(query: &QC, key: Self::Key) -> Self::Value;
}

pub trait QueryStorageOps<QC, Q>: Default
where
    QC: BaseQueryContext,
    Q: Query<QC>,
{
    fn try_fetch<'q>(
        &self,
        query: &'q QC,
        key: &Q::Key,
        descriptor: impl FnOnce() -> QC::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected>;
}

#[derive(new)]
pub struct QueryTable<'me, QC, Q>
where
    QC: BaseQueryContext,
    Q: Query<QC>,
{
    pub query: &'me QC,
    pub storage: &'me Q::Storage,
    pub descriptor_fn: fn(&QC, &Q::Key) -> QC::QueryDescriptor,
}

#[derive(Debug)]
pub enum QueryState<V> {
    InProgress,
    Memoized(V),
}

pub struct CycleDetected;

impl<QC, Q> QueryTable<'me, QC, Q>
where
    QC: BaseQueryContext,
    Q: Query<QC>,
{
    pub fn of(&self, key: Q::Key) -> Q::Value {
        self.storage
            .try_fetch(self.query, &key, || self.descriptor(&key))
            .unwrap_or_else(|CycleDetected| {
                self.query.report_unexpected_cycle(self.descriptor(&key))
            })
    }

    fn descriptor(&self, key: &Q::Key) -> QC::QueryDescriptor {
        (self.descriptor_fn)(self.query, key)
    }
}

/// A macro helper for writing the query contexts in traits that helps
/// you avoid repeating information.
///
/// Example:
///
/// ```ignore
/// trait TypeckQueryContext {
///     query_prototype!(fn <method>() for <type>);
/// }
/// ```
#[macro_export]
macro_rules! query_prototype {
    (
        $(#[$attr:meta])*
        fn $method_name:ident() for $query_type:ty
    ) => {
        $(#[$attr])*
        fn $method_name(&self) -> $crate::QueryTable<'_, Self, $query_type>;
    }
}

/// Example:
///
/// ```ignore
/// query_definition! {
///     QueryName(query: &impl TypeckQueryContext, key: DefId) -> Arc<Vec<DefId>> {
///         ...
///     }
/// }
/// ```
#[macro_export]
macro_rules! query_definition {
    (
        $(#[$attr:meta])*
        $v:vis $name:ident($query:tt : &impl $query_trait:path, $key:tt : $key_ty:ty) -> $value_ty:ty {
            $($body:tt)*
        }
    ) => {
        #[derive(Default, Debug)]
        $v struct $name;

        impl<QC> $crate::Query<QC> for $name
        where
            QC: $query_trait,
        {
            type Key = $key_ty;
            type Value = $value_ty;
            type Storage = $crate::storage::MemoizedStorage<QC, Self>;

            fn execute($query: &QC, $key: $key_ty) -> $value_ty {
                $($body)*
            }
        }
    }
}
