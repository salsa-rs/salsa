//! Public API facades for the implementation details of [`Zalsa`] and [`ZalsaLocal`].
use std::marker::PhantomData;
use std::mem;
use std::panic::RefUnwindSafe;

use crate::database::RawDatabase;
use crate::sync::{Arc, Condvar, Mutex};
use crate::zalsa::{ErasedJar, HasJar, Zalsa, ZalsaDatabase};
use crate::zalsa_local::{self, ZalsaLocal};
use crate::{Database, Event, EventKind};

/// A handle to non-local database state.
pub struct StorageHandle<Db> {
    // Note: Drop order is important, zalsa_impl needs to drop before coordinate
    /// Reference to the database.
    zalsa_impl: Arc<Zalsa>,

    // Note: Drop order is important, coordinate needs to drop after zalsa_impl
    /// Coordination data for cancellation of other handles when `zalsa_mut` is called.
    /// This could be stored in Zalsa but it makes things marginally cleaner to keep it separate.
    coordinate: CoordinateDrop,

    /// We store references to `Db`
    phantom: PhantomData<fn() -> Db>,
}

impl<Db> Clone for StorageHandle<Db> {
    fn clone(&self) -> Self {
        Self {
            zalsa_impl: self.zalsa_impl.clone(),
            coordinate: CoordinateDrop(Arc::clone(&self.coordinate)),
            phantom: PhantomData,
        }
    }
}

impl<Db: Database> Default for StorageHandle<Db> {
    fn default() -> Self {
        Self::new(None)
    }
}

impl<Db: Database> StorageHandle<Db> {
    pub fn new(event_callback: Option<Box<dyn Fn(crate::Event) + Send + Sync + 'static>>) -> Self {
        Self::with_jars(event_callback, Vec::new())
    }

    fn with_jars(
        event_callback: Option<Box<dyn Fn(crate::Event) + Send + Sync + 'static>>,
        jars: Vec<ErasedJar>,
    ) -> Self {
        Self {
            zalsa_impl: Arc::new(Zalsa::new::<Db>(event_callback, jars)),
            coordinate: CoordinateDrop(Arc::new(Coordinate {
                coordinate_lock: Mutex::default(),
                cvar: Default::default(),
            })),
            phantom: PhantomData,
        }
    }

    pub fn into_storage(self) -> Storage<Db> {
        Storage {
            handle: self,
            zalsa_local: ZalsaLocal::new(),
        }
    }
}

/// Access the "storage" of a Salsa database: this is an internal plumbing trait
/// automatically implemented by `#[salsa::db]` applied to a struct.
///
/// # Safety
///
/// The `storage` and `storage_mut` fields must both return a reference to the same
/// storage field which must be owned by `self`.
pub unsafe trait HasStorage: Database + Clone + Sized {
    fn storage(&self) -> &Storage<Self>;
    fn storage_mut(&mut self) -> &mut Storage<Self>;
}

/// Concrete implementation of the [`Database`] trait with local state that can be used to drive computations.
pub struct Storage<Db> {
    handle: StorageHandle<Db>,

    /// Per-thread state
    zalsa_local: zalsa_local::ZalsaLocal,
}

impl<Db> Drop for Storage<Db> {
    fn drop(&mut self) {
        self.zalsa_local
            .record_unfilled_pages(self.handle.zalsa_impl.table());
    }
}

impl<Db: Database> Default for Storage<Db> {
    fn default() -> Self {
        Self::new(None)
    }
}

impl<Db: Database> Storage<Db> {
    /// Create a new database storage.
    ///
    /// The `event_callback` function is invoked by the salsa runtime at various points during execution.
    pub fn new(event_callback: Option<Box<dyn Fn(crate::Event) + Send + Sync + 'static>>) -> Self {
        Self {
            handle: StorageHandle::new(event_callback),
            zalsa_local: ZalsaLocal::new(),
        }
    }

    /// Returns a builder for database storage.
    pub fn builder() -> StorageBuilder<Db> {
        StorageBuilder::default()
    }

    /// Convert this instance of [`Storage`] into a [`StorageHandle`].
    ///
    /// This will discard the local state of this [`Storage`], thereby returning a value that
    /// is both [`Sync`] and [`std::panic::UnwindSafe`].
    pub fn into_zalsa_handle(mut self) -> StorageHandle<Db> {
        self.zalsa_local
            .record_unfilled_pages(self.handle.zalsa_impl.table());
        let Self {
            handle,
            zalsa_local: _,
        } = &self;
        // Avoid rust's annoying destructure prevention rules for `Drop` types
        // SAFETY: We forget `Self` afterwards to discard the original copy, and the destructure
        // above makes sure we won't forget to take into account newly added fields.
        let handle = unsafe { std::ptr::read(handle) };
        std::mem::forget::<Self>(self);
        handle
    }

    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    ///
    /// Needs to be paired with a call to `reset_cancellation_flag`.
    fn cancel_others(&mut self) -> &mut Zalsa {
        debug_assert!(
            self.zalsa_local
                .try_with_query_stack(|stack| stack.is_empty())
                == Some(true),
            "attempted to cancel within query computation, this is a deadlock"
        );
        self.handle.zalsa_impl.runtime().set_cancellation_flag();

        self.handle
            .zalsa_impl
            .event(&|| Event::new(EventKind::DidSetCancellationFlag));

        let mut coordinate_lock = self.handle.coordinate.coordinate_lock.lock();
        let zalsa = loop {
            if let Some(zalsa) = Arc::get_mut(&mut self.handle.zalsa_impl) {
                // SAFETY: Polonius when ... https://github.com/rust-lang/rfcs/blob/master/text/2094-nll.md#problem-case-3-conditional-control-flow-across-functions
                break unsafe { mem::transmute::<&mut Zalsa, &mut Zalsa>(zalsa) };
            }
            coordinate_lock = self.handle.coordinate.cvar.wait(coordinate_lock);
        };
        // cancellation is done, so reset the flag
        zalsa.runtime_mut().reset_cancellation_flag();
        zalsa
    }
    // ANCHOR_END: cancel_other_workers
}

/// A builder for a [`Storage`] instance.
///
/// This type can be created with the [`Storage::builder`] function.
pub struct StorageBuilder<Db> {
    jars: Vec<ErasedJar>,
    event_callback: Option<Box<dyn Fn(crate::Event) + Send + Sync + 'static>>,
    _db: PhantomData<Db>,
}

impl<Db> Default for StorageBuilder<Db> {
    fn default() -> Self {
        Self {
            jars: Vec::new(),
            event_callback: None,
            _db: PhantomData,
        }
    }
}

impl<Db: Database> StorageBuilder<Db> {
    /// Set a callback for salsa events.
    ///
    /// The `event_callback` function will be invoked by the salsa runtime at various points during execution.
    pub fn event_callback(
        mut self,
        callback: Box<dyn Fn(crate::Event) + Send + Sync + 'static>,
    ) -> Self {
        self.event_callback = Some(callback);
        self
    }

    /// Manually register an ingredient.
    ///
    /// Manual ingredient registration is necessary when the `inventory` feature is disabled.
    pub fn ingredient<I: HasJar>(mut self) -> Self {
        self.jars.push(ErasedJar::erase::<I>());
        self
    }

    /// Construct the [`Storage`] using the provided builder options.
    pub fn build(self) -> Storage<Db> {
        Storage {
            handle: StorageHandle::with_jars(self.event_callback, self.jars),
            zalsa_local: ZalsaLocal::new(),
        }
    }
}

#[allow(clippy::undocumented_unsafe_blocks)] // TODO(#697) document safety
unsafe impl<T: HasStorage> ZalsaDatabase for T {
    #[inline(always)]
    fn zalsa(&self) -> &Zalsa {
        &self.storage().handle.zalsa_impl
    }

    fn zalsa_mut(&mut self) -> &mut Zalsa {
        self.storage_mut().cancel_others()
    }

    #[inline(always)]
    fn zalsa_local(&self) -> &ZalsaLocal {
        &self.storage().zalsa_local
    }

    #[inline(always)]
    fn fork_db(&self) -> RawDatabase<'static> {
        Box::leak(Box::new(self.clone())).into()
    }
}

impl<Db: Database> Clone for Storage<Db> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            zalsa_local: ZalsaLocal::new(),
        }
    }
}

/// A simplified `WaitGroup`, this is used together with `Arc<Zalsa>` as the actual counter
struct Coordinate {
    coordinate_lock: Mutex<()>,
    cvar: Condvar,
}

// We cannot panic while holding a lock to `clones: Mutex<usize>` and therefore we cannot enter an
// inconsistent state.
impl RefUnwindSafe for Coordinate {}

struct CoordinateDrop(Arc<Coordinate>);

impl std::ops::Deref for CoordinateDrop {
    type Target = Arc<Coordinate>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for CoordinateDrop {
    fn drop(&mut self) {
        self.0.cvar.notify_all();
    }
}
