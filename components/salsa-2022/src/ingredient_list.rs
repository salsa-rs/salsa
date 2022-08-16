use std::sync::Arc;

use arc_swap::{ArcSwapOption, AsRaw};

use crate::IngredientIndex;

/// A list of ingredients that can be added to in parallel.
pub(crate) struct IngredientList {
    /// A list of each tracked functions.
    /// tracked struct.
    ///
    /// Whenever an instance `i` of this struct is deleted,
    /// each of these functions will be notified
    /// so they can remove any data tied to that instance.
    list: ArcSwapOption<Vec<IngredientIndex>>,
}

impl IngredientList {
    pub fn new() -> Self {
        Self {
            list: ArcSwapOption::new(None),
        }
    }

    /// Returns an iterator over the items in the list.
    /// This is a snapshot of the list as it was when this function is called.
    /// Items could still be added in parallel via `add_ingredient`
    /// that will not be returned by this iterator.
    pub(crate) fn iter(&self) -> impl Iterator<Item = IngredientIndex> {
        let guard = self.list.load();
        let mut index = 0;
        std::iter::from_fn(move || match &*guard {
            Some(list) if index < list.len() => {
                let r = list[index];
                index += 1;
                Some(r)
            }
            _ => None,
        })
    }

    /// Adds an ingredient to the list (if not already present).
    pub(crate) fn push(&self, index: IngredientIndex) {
        // This function is called whenever a value is stored,
        // so other tracked functions and things may be executing,
        // and there could even be two calls to this function in parallel.
        //
        // We use a "compare-and-swap" strategy of reading the old vector, creating a new vector,
        // and then installing it, hoping that nobody has conflicted with us.
        // If that fails, we start over.

        loop {
            let guard = self.list.load();
            let empty_vec = vec![];
            let old_vec = match &*guard {
                Some(v) => v,
                None => &empty_vec,
            };

            // First check whether the index is already present.
            if old_vec.contains(&index) {
                return;
            }

            // If not, construct a new vector that has all the old values, followed by `index`.
            let vec: Arc<Vec<IngredientIndex>> = Arc::new(
                old_vec
                    .iter()
                    .copied()
                    .chain(std::iter::once(index))
                    .collect(),
            );

            // Try to replace the old vector with the new one. If we fail, loop around again.
            assert_eq!(vec.len(), vec.capacity());
            let previous = self.list.compare_and_swap(&guard, Some(vec));
            if guard.as_raw() == previous.as_raw() {
                // swap was successful
                break;
            }
        }
    }
}
