#![warn(rust_2018_idioms)]
#![allow(dead_code)]

mod derived;
mod input;
mod runtime;

pub mod debug;
/// Items in this module are public for implementation reasons,
/// and are exempt from the SemVer guarantees.
#[doc(hidden)]
pub mod plumbing;

use crate::plumbing::CycleDetected;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::plumbing::UncheckedMutQueryStorageOps;
use derive_new::new;
use std::fmt::{self, Debug};
use std::hash::Hash;

pub use crate::runtime::Frozen;
pub use crate::runtime::Runtime;
pub use crate::runtime::RuntimeId;

/// The base trait which your "query context" must implement. Gives
/// access to the salsa runtime, which you must embed into your query
/// context (along with whatever other state you may require).
pub trait Database: plumbing::DatabaseStorageTypes + plumbing::DatabaseOps {
    /// Gives access to the underlying salsa runtime.
    fn salsa_runtime(&self) -> &Runtime<Self>;

    /// Iterates through all query storage and removes any values that
    /// have not been used since the last revision was created. The
    /// intended use-cycle is that you first execute all of your
    /// "main" queries; this will ensure that all query values they
    /// consume are marked as used.  You then invoke this method to
    /// remove other values that were not needed for your main query
    /// results.
    ///
    /// **A note on atomicity.** Since `sweep_all` removes data that
    /// hasn't been actively used since the last revision, executing
    /// `sweep_all` concurrently with `set` operations is not
    /// recommended.  It won't do any *harm* -- but it may cause more
    /// data to be discarded then you expect, leading to more
    /// re-computation. You can use the [`lock_revision`][] method to
    /// guarantee atomicity (or execute `sweep_all` from the same
    /// threads that would be performing a `set`).
    ///
    /// [`lock_revision`]: struct.Runtime.html#method.lock_revision
    fn sweep_all(&self, strategy: SweepStrategy) {
        self.salsa_runtime().sweep_all(self, strategy);
    }

    /// Get access to extra methods pertaining to a given query,
    /// notably `set` (for inputs).
    #[allow(unused_variables)]
    fn query<Q>(&self, query: Q) -> QueryTable<'_, Self, Q>
    where
        Q: Query<Self>,
        Self: plumbing::GetQueryTable<Q>,
    {
        <Self as plumbing::GetQueryTable<Q>>::get_query_table(self)
    }

    /// This function is invoked at key points in the salsa
    /// runtime. It permits the database to be customized and to
    /// inject logging or other custom behavior.
    fn salsa_event(&self, event_fn: impl Fn() -> Event<Self>) {
        #![allow(unused_variables)]
    }
}

pub struct Event<DB: Database> {
    pub runtime_id: RuntimeId,
    pub kind: EventKind<DB>,
}

impl<DB: Database> fmt::Debug for Event<DB> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Event")
            .field("runtime_id", &self.runtime_id)
            .field("kind", &self.kind)
            .finish()
    }
}

pub enum EventKind<DB: Database> {
    /// Occurs when we found that all inputs to a memoized value are
    /// up-to-date and hence the value can be re-used without
    /// executing the closure.
    ///
    /// Executes before the "re-used" value is returned.
    DidValidateMemoizedValue { descriptor: DB::QueryDescriptor },

    /// Indicates that another thread (with id `other_runtime_id`) is processing the
    /// given query (`descriptor`), so we will block until they
    /// finish.
    ///
    /// Executes after we have registered with the other thread but
    /// before they have answered us.
    ///
    /// (NB: you can find the `id` of the current thread via the
    /// `salsa_runtime`)
    WillBlockOn {
        other_runtime_id: RuntimeId,
        descriptor: DB::QueryDescriptor,
    },

    /// Indicates that the input value will change after this
    /// callback, e.g. due to a call to `set`.
    WillChangeInputValue { descriptor: DB::QueryDescriptor },

    /// Indicates that the function for this query will be executed.
    /// This is either because it has never executed before or because
    /// its inputs may be out of date.
    WillExecute { descriptor: DB::QueryDescriptor },
}

impl<DB: Database> fmt::Debug for EventKind<DB> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventKind::DidValidateMemoizedValue { descriptor } => fmt
                .debug_struct("DidValidateMemoizedValue")
                .field("descriptor", descriptor)
                .finish(),
            EventKind::WillBlockOn {
                other_runtime_id,
                descriptor,
            } => fmt
                .debug_struct("WillBlockOn")
                .field("other_runtime_id", other_runtime_id)
                .field("descriptor", descriptor)
                .finish(),
            EventKind::WillChangeInputValue { descriptor } => fmt
                .debug_struct("WillChangeInputValue")
                .field("descriptor", descriptor)
                .finish(),
            EventKind::WillExecute { descriptor } => fmt
                .debug_struct("WillExecute")
                .field("descriptor", descriptor)
                .finish(),
        }
    }
}

/// The sweep strategy controls what data we will keep/discard when we
/// do a GC-sweep. The default (`SweepStrategy::default`) is to keep
/// all memoized values used in the current revision.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SweepStrategy {
    keep_values: bool,
}

impl SweepStrategy {
    /// Causes us to discard memoized *values* but keep the
    /// *dependencies*. This means you will have to recompute the
    /// results from any queries you execute but does permit you to
    /// quickly determine if a value is still up to date.
    pub fn discard_values(self) -> SweepStrategy {
        SweepStrategy {
            keep_values: false,
            ..self
        }
    }
}

impl Default for SweepStrategy {
    fn default() -> Self {
        SweepStrategy { keep_values: true }
    }
}

/// Indicates a database that also supports parallel query
/// evaluation. All of Salsa's base query support is capable of
/// parallel execution, but for it to work, your query key/value types
/// must also be `Send`, as must any additional data in your database.
pub trait ParallelDatabase: Database + Send {
    /// Creates a second handle to the database that holds the
    /// database fixed at a particular revision. So long as this
    /// "frozen" handle exists, any attempt to [`set`] an input will
    /// block.
    ///
    /// [`set`]: struct.QueryTable.html#method.set
    ///
    /// This is the method you are meant to use most of the time in a
    /// parallel setting where modifications may arise asynchronously
    /// (e.g., a language server). In this context, it is common to
    /// wish to "fork off" a copy of the database performing some
    /// series of queries in parallel and arranging the results. Using
    /// this method for that purpose ensures that those queries will
    /// see a consistent view of the database (it is also advisable
    /// for those queries to use the [`is_current_revision_canceled`]
    /// method to check for cancellation).
    ///
    /// [`is_current_revision_canceled`]: struct.Runtime.html#method.is_current_revision_canceled
    ///
    /// # Deadlock warning
    ///
    /// This second handle is intended to be used from a separate
    /// thread. Using two database handles from the **same thread**
    /// can lead to deadlock.
    fn fork(&self) -> Frozen<Self> {
        Frozen::new(self)
    }

    /// Creates another handle to this database destined for another
    /// thread. You are meant to implement this by cloning a second
    /// handle to all all the state in the database; to clone the
    /// salsa runtime, use the [`Runtime::fork_mut`] method.
    ///
    /// Note to users: you only need this method if you plan to be
    /// setting inputs on the other thread. Otherwise, it is preferred
    /// to use [`fork`], which will given back a frozen database fixed
    /// at the current revision.
    ///
    /// # Deadlock warning
    ///
    /// This second handle is intended to be used from a
    /// separate thread. Using two database handles from the **same
    /// thread** can lead to deadlock.
    ///
    /// [`Runtime::fork_mut`]: https://docs.rs/salsa/0.7.0/salsa/struct.Runtime.html#method.fork_mut
    /// [`fork`]: trait.ParallelDatabase.html#tymethod.fork
    fn fork_mut(&self) -> Self;
}

pub trait Query<DB: Database>: Debug + Default + Sized + 'static {
    type Key: Clone + Debug + Hash + Eq;
    type Value: Clone + Debug;
    type Storage: plumbing::QueryStorageOps<DB, Self> + Send + Sync;
}

#[derive(new)]
pub struct QueryTable<'me, DB, Q>
where
    DB: Database + 'me,
    Q: Query<DB> + 'me,
{
    db: &'me DB,
    storage: &'me Q::Storage,
    descriptor_fn: fn(&DB, &Q::Key) -> DB::QueryDescriptor,
}

impl<DB, Q> QueryTable<'_, DB, Q>
where
    DB: Database,
    Q: Query<DB>,
{
    pub fn get(&self, key: Q::Key) -> Q::Value {
        let descriptor = self.descriptor(&key);
        self.storage
            .try_fetch(self.db, &key, &descriptor)
            .unwrap_or_else(|CycleDetected| {
                self.db.salsa_runtime().report_unexpected_cycle(descriptor)
            })
    }

    pub fn sweep(&self, strategy: SweepStrategy)
    where
        Q::Storage: plumbing::QueryStorageMassOps<DB>,
    {
        self.storage.sweep(self.db, strategy);
    }

    /// Assign a value to an "input query". Must be used outside of
    /// an active query computation.
    pub fn set(&self, key: Q::Key, value: Q::Value)
    where
        Q::Storage: plumbing::InputQueryStorageOps<DB, Q>,
    {
        self.storage
            .set(self.db, &key, &self.descriptor(&key), value);
    }

    /// Assign a value to an "input query", with the additional
    /// promise that this value will **never change**. Must be used
    /// outside of an active query computation.
    pub fn set_constant(&self, key: Q::Key, value: Q::Value)
    where
        Q::Storage: plumbing::InputQueryStorageOps<DB, Q>,
    {
        self.storage
            .set_constant(self.db, &key, &self.descriptor(&key), value);
    }

    /// Assigns a value to the query **bypassing the normal
    /// incremental checking** -- this value becomes the value for the
    /// query in the current revision. This can even be used on
    /// "derived" queries (so long as their results are memoized).
    ///
    /// Note that once `set_unchecked` is used, the result is
    /// effectively "fixed" for all future revisions. This "mocking"
    /// system is pretty primitive and subject to revision; see
    /// [salsa-rs/salsa#34](https://github.com/salsa-rs/salsa/issues/34)
    /// for more details.
    ///
    /// **This is only meant to be used for "mocking" purposes in
    /// tests** -- when testing a given query, you can use
    /// `set_unchecked` to assign the values for its various inputs
    /// and thus control what it sees when it executes.
    pub fn set_unchecked(&self, key: Q::Key, value: Q::Value)
    where
        Q::Storage: plumbing::UncheckedMutQueryStorageOps<DB, Q>,
    {
        self.storage.set_unchecked(self.db, &key, value);
    }

    fn descriptor(&self, key: &Q::Key) -> DB::QueryDescriptor {
        (self.descriptor_fn)(self.db, key)
    }
}

/// A macro that helps in defining the "context trait" of a given
/// module.  This is a trait that defines everything that a block of
/// queries need to execute, as well as defining the queries
/// themselves that are exported for others to use.
///
/// This macro declares the "prototype" for a group of queries. It will
/// expand into a trait and a set of structs, one per query.
///
/// For each query, you give the name of the accessor method to invoke
/// the query (e.g., `my_query`, below), as well as its parameter
/// types and the output type. You also give the name for a query type
/// (e.g., `MyQuery`, below) that represents the query, and optionally
/// other details, such as its storage.
///
/// # Examples
///
/// The simplest example is something like this:
///
/// ```ignore
/// trait TypeckDatabase {
///     query_group! {
///         /// Comments or other attributes can go here
///         fn my_query(input: u32) -> u64 {
///             type MyQuery;
///             storage memoized; // optional, this is the default
///             use fn path::to::fn; // optional, default is `my_query`
///         }
///
///         /// Queries can have any number of inputs; the key type will be
///         /// a tuple of the input types, so in this case `(u32, f32)`.
///         fn other_query(input1: u32, input2: f32) -> u64 {
///             type OtherQuery;
///         }
///     }
/// }
/// ```
///
/// # Storage modes
///
/// Here are the possible storage values for each query.  The default
/// is `storage memoized`.
///
/// ## Input queries
///
/// Specifying `storage input` will give you an **input
/// query**. Unlike derived queries, whose value is given by a
/// function, input queries are explicit set by doing
/// `db.query(QueryType).set(key, value)` (where `QueryType` is the
/// `type` specified for the query). Accessing a value that has not
/// yet been set will panic. Each time you invoke `set`, we assume the
/// value has changed, and so we will potentially re-execute derived
/// queries that read (transitively) from this input.
///
/// ## Derived queries
///
/// Derived queries are specified by a function.
///
/// - `storage memoized` -- The result is memoized
///   between calls.  If the inputs have changed, we will recompute
///   the value, but then compare against the old memoized value,
///   which can significantly reduce the amount of recomputation
///   required in new revisions. This does require that the value
///   implements `Eq`.
/// - `storage volatile` -- indicates that the inputs are not fully
///   captured by salsa. The result will be recomputed once per revision.
/// - `storage dependencies` -- does not cache the value, so it will
///   be recomputed every time it is needed. We do track the inputs, however,
///   so if they have not changed, then things that rely on this query
///   may be known not to have changed.
#[macro_export]
macro_rules! query_group {
    (
        $(#[$attr:meta])* $v:vis trait $name:ident { $($t:tt)* }
    ) => {
        $crate::query_group! {
            attr[$(#[$attr])*];
            headers[$v, $name, ];
            tokens[{ $($t)* }];
        }
    };

    (
        $(#[$attr:meta])* $v:vis trait $name:ident : $($t:tt)*
    ) => {
        $crate::query_group! {
            attr[$(#[$attr])*];
            headers[$v, $name, ];
            tokens[$($t)*];
        }
    };

    // Base case: found the trait body
    (
        attr[$($trait_attr:tt)*];
        headers[$v:vis, $query_trait:ident, $($header:tt)*];
        tokens[{
            $(
                $(#[$method_attr:meta])*
                fn $method_name:ident($($key_name:ident: $key_ty:ty),* $(,)*) -> $value_ty:ty {
                    type $QueryType:ident;
                    $(storage $storage:tt;)* // FIXME(rust-lang/rust#48075) should be `?`
                    $(use fn $fn_path:path;)* // FIXME(rust-lang/rust#48075) should be `?`
                }
            )*
        }];
    ) => {
        $($trait_attr)* $v trait $query_trait: $($crate::plumbing::GetQueryTable<$QueryType> +)* $($header)* {
            $(
                $(#[$method_attr])*
                fn $method_name(&self, $($key_name: $key_ty),*) -> $value_ty {
                    <Self as $crate::plumbing::GetQueryTable<$QueryType>>::get_query_table(self)
                        .get(($($key_name),*))
                }
            )*
        }

        $(
            #[derive(Default, Debug)]
            $v struct $QueryType;

            impl<DB> $crate::Query<DB> for $QueryType
            where
                DB: $query_trait,
            {
                type Key = ($($key_ty),*);
                type Value = $value_ty;
                type Storage = $crate::query_group! { @storage_ty[DB, Self, $($storage)*] };
            }

            $crate::query_group! {
                @query_fn[
                    storage($($storage)*);
                    method_name($method_name);
                    fn_path($($fn_path)*);
                    db_trait($query_trait);
                    query_type($QueryType);
                    key($($key_name: $key_ty),*);
                ]
            }
        )*
    };

    (
        @query_fn[
            storage(input);
            method_name($method_name:ident);
            fn_path();
            $($rest:tt)*
        ]
    ) => {
        // do nothing for `storage input`, presuming they did not write an explicit `use fn`
    };

    (
        @query_fn[
            storage(input);
            method_name($method_name:ident);
            fn_path($fn_path:path);
            $($rest:tt)*
        ]
    ) => {
        // error for `storage input` with an explicit `use fn`
        compile_error! {
            "cannot have `storage input` combined with `use fn`"
        }
    };

    (
        @query_fn[
            storage($($storage:ident)*);
            method_name($method_name:ident);
            fn_path();
            $($rest:tt)*
        ]
    ) => {
        // default to `use fn $method_name`
        $crate::query_group! {
            @query_fn[
                storage($($storage)*);
                method_name($method_name);
                fn_path($method_name);
                $($rest)*
            ]
        }
    };

    // Handle fns of one argument: once parenthesized patterns are stable on beta,
    // we can remove this special case.
    (
        @query_fn[
            storage($($storage:ident)*);
            method_name($method_name:ident);
            fn_path($fn_path:path);
            db_trait($DbTrait:path);
            query_type($QueryType:ty);
            key($key_name:ident: $key_ty:ty);
        ]
    ) => {
        impl<DB> $crate::plumbing::QueryFunction<DB> for $QueryType
        where DB: $DbTrait
        {
            fn execute(db: &DB, $key_name: <Self as $crate::Query<DB>>::Key)
                       -> <Self as $crate::Query<DB>>::Value
            {
                $fn_path(db, $key_name)
            }
        }
    };

    // Handle fns of N arguments: once parenthesized patterns are stable on beta,
    // we can use this code for all cases.
    (
        @query_fn[
            storage($($storage:ident)*);
            method_name($method_name:ident);
            fn_path($fn_path:path);
            db_trait($DbTrait:path);
            query_type($QueryType:ty);
            key($($key_name:ident: $key_ty:ty),*);
        ]
    ) => {
        impl<DB> $crate::plumbing::QueryFunction<DB> for $QueryType
        where DB: $DbTrait
        {
            fn execute(db: &DB, ($($key_name),*): <Self as $crate::Query<DB>>::Key)
                       -> <Self as $crate::Query<DB>>::Value
            {
                $fn_path(db, $($key_name),*)
            }
        }
    };

    // Recursive case: found some more part of the trait header.
    // Keep pulling out tokens until we find the body.
    (
        attr[$($attr:tt)*];
        headers[$($headers:tt)*];
        tokens[$token:tt $($tokens:tt)*];
    ) => {
        $crate::query_group! {
            attr[$($attr)*];
            headers[$($headers)* $token];
            tokens[$($tokens)*];
        }
    };

    // Generate storage type
    (
        // Default case:
        @storage_ty[$DB:ident, $Self:ident, ]
    ) => {
        $crate::query_group! { @storage_ty[$DB, $Self, memoized] }
    };

    (
        @storage_ty[$DB:ident, $Self:ident, memoized]
    ) => {
        $crate::plumbing::MemoizedStorage<$DB, $Self>
    };

    (
        @storage_ty[$DB:ident, $Self:ident, volatile]
    ) => {
        $crate::plumbing::VolatileStorage<$DB, $Self>
    };

    (
        @storage_ty[$DB:ident, $Self:ident, dependencies]
    ) => {
        $crate::plumbing::DependencyStorage<$DB, $Self>
    };

    (
        @storage_ty[$DB:ident, $Self:ident, input]
    ) => {
        $crate::plumbing::InputStorage<DB, Self>
    };

    (
        @storage_ty[$DB:ident, $Self:ident, $storage:tt]
    ) => {
        compile_error! {
            "invalid storage specification"
        }
    };
}

/// This macro generates the "query storage" that goes into your query
/// context.
#[macro_export]
macro_rules! database_storage {
    (
        $(#[$attr:meta])*
        $v:vis struct $Storage:ident for $Database:ty {
            $(
                impl $TraitName:path {
                    $(
                        fn $query_method:ident() for $QueryType:path;
                    )*
                }
            )*
        }
    ) => {
        #[derive(Default)]
        $(#[$attr])*
        $v struct $Storage {
            $(
                $(
                    $query_method: <$QueryType as $crate::Query<$Database>>::Storage,
                )*
            )*
        }

        /// Identifies a query and its key. You are not meant to name
        /// this type directly or use its fields etc.  It is a
        /// **private query descriptor type generated by salsa** and
        /// its exact structure is subject to change. Sadly, I don't
        /// know any way to hide this with hygiene, so use `__`
        /// instead.
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        $v struct __SalsaQueryDescriptor {
            kind: __SalsaQueryDescriptorKind
        }

        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        enum __SalsaQueryDescriptorKind {
            $(
                $(
                    $query_method(<$QueryType as $crate::Query<$Database>>::Key),
                )*
            )*
        }

        impl $crate::plumbing::DatabaseStorageTypes for $Database {
            type QueryDescriptor = __SalsaQueryDescriptor;
            type DatabaseStorage = $Storage;
        }

        impl $crate::plumbing::DatabaseOps for $Database {
            fn for_each_query(
                &self,
                mut op: impl FnMut(&dyn $crate::plumbing::QueryStorageMassOps<Self>),
            ) {
                $(
                    $(
                        op(&$crate::Database::salsa_runtime(self)
                           .storage()
                           .$query_method);
                    )*
                )*
            }
        }

        impl $crate::plumbing::QueryDescriptor<$Database> for __SalsaQueryDescriptor {
            fn maybe_changed_since(
                &self,
                db: &$Database,
                revision: $crate::plumbing::Revision,
            ) -> bool {
                match &self.kind {
                    $(
                        $(
                            __SalsaQueryDescriptorKind::$query_method(key) => {
                                let runtime = $crate::Database::salsa_runtime(db);
                                let storage = &runtime.storage().$query_method;
                                <_ as $crate::plumbing::QueryStorageOps<$Database, $QueryType>>::maybe_changed_since(
                                    storage,
                                    db,
                                    revision,
                                    key,
                                    self,
                                )
                            }
                        )*
                    )*
                }
            }
        }

        $(
            impl $TraitName for $Database { }

            $(
                impl $crate::plumbing::GetQueryTable<$QueryType> for $Database {
                    fn get_query_table(
                        db: &Self,
                    ) -> $crate::QueryTable<'_, Self, $QueryType> {
                        $crate::QueryTable::new(
                            db,
                            &$crate::Database::salsa_runtime(db)
                                .storage()
                                .$query_method,
                            |_, key| {
                                let key = std::clone::Clone::clone(key);
                                __SalsaQueryDescriptor {
                                    kind: __SalsaQueryDescriptorKind::$query_method(key),
                                }
                            },
                        )
                    }
                }
            )*
        )*
    };
}
