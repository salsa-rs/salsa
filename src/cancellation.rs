use std::fmt;

use crate::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::{Arc, Mutex};

/// Identifies a registered revision-cancellation callback.
///
/// Create a registration with [`crate::Database::on_cancellation`] and remove it by calling
/// [`Self::unregister`]. Dropping the registration does not remove the callback.
#[must_use = "keep the registration to unregister its cancellation callback"]
pub struct CancellationRegistration {
    callbacks: Arc<Mutex<Vec<RegisteredCallback>>>,
    id: usize,
}

impl CancellationRegistration {
    /// Unregisters the callback if it is still registered.
    ///
    /// A callback already running on another thread may finish after this method returns.
    pub fn unregister(&self) {
        let callback = {
            let mut callbacks = self.callbacks.lock();
            callbacks
                .iter()
                .position(|callback| callback.id == self.id)
                .map(|index| callbacks.swap_remove(index).callback)
        };

        drop(callback);
    }

    pub(crate) fn notify(&self) {
        let callback = self
            .callbacks
            .lock()
            .iter()
            .find(|callback| callback.id == self.id)
            .map(|callback| Arc::clone(&callback.callback));

        if let Some(callback) = callback {
            callback();
        }
    }
}

impl fmt::Debug for CancellationRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CancellationRegistration")
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
pub(crate) struct CancellationCallbacks {
    callbacks: Arc<Mutex<Vec<RegisteredCallback>>>,
    next_id: AtomicUsize,
}

impl CancellationCallbacks {
    pub(crate) fn register(
        &self,
        callback: Box<dyn Fn() + Send + Sync + 'static>,
    ) -> CancellationRegistration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.callbacks.lock().push(RegisteredCallback {
            id,
            callback: Arc::from(callback),
        });

        CancellationRegistration {
            callbacks: Arc::clone(&self.callbacks),
            id,
        }
    }

    pub(crate) fn notify(&self) {
        let callbacks = self
            .callbacks
            .lock()
            .iter()
            .map(|callback| Arc::clone(&callback.callback))
            .collect::<Vec<_>>();

        for callback in callbacks {
            callback();
        }
    }
}

struct RegisteredCallback {
    id: usize,
    callback: Arc<dyn Fn() + Send + Sync + 'static>,
}
