use std::ops::{BitOr, BitOrAssign};

use crossbeam::atomic::AtomicCell;
use rustc_hash::FxHashMap;

use crate::IngredientIndex;

use super::{accumulated::Accumulated, Accumulator, AnyAccumulated};

#[derive(Default, Debug)]
pub struct AccumulatedMap {
    map: FxHashMap<IngredientIndex, Box<dyn AnyAccumulated>>,

    /// [`InputAccumulatedValues::Empty`] if any input read during the query's execution
    /// has any direct or indirect accumulated values.
    inputs: AtomicCell<InputAccumulatedValues>,
}

impl AccumulatedMap {
    pub fn accumulate<A: Accumulator>(&mut self, index: IngredientIndex, value: A) {
        self.map
            .entry(index)
            .or_insert_with(|| <Box<Accumulated<A>>>::default())
            .accumulate(value);
    }

    /// Adds the accumulated state of an input to this accumulated map.
    pub(crate) fn add_input(&self, input: InputAccumulatedValues) {
        if input.is_any() {
            self.inputs.store(InputAccumulatedValues::Any);
        }
    }

    pub(crate) fn set_inputs(&self, input: InputAccumulatedValues) {
        self.inputs.store(input);
    }

    /// Returns whether an input of the associated query has any accumulated values.
    ///
    /// Note: Use [`InputAccumulatedValues::from_map`] to check if the associated query itself
    /// or any of its inputs has accumulated values.
    pub(crate) fn inputs(&self) -> InputAccumulatedValues {
        self.inputs.load()
    }

    pub fn extend_with_accumulated<A: Accumulator>(
        &self,
        index: IngredientIndex,
        output: &mut Vec<A>,
    ) {
        let Some(a) = self.map.get(&index) else {
            return;
        };

        a.as_dyn_any()
            .downcast_ref::<Accumulated<A>>()
            .unwrap()
            .extend_with_accumulated(output);
    }
}

impl Clone for AccumulatedMap {
    fn clone(&self) -> Self {
        Self {
            map: self
                .map
                .iter()
                .map(|(&key, value)| (key, value.cloned()))
                .collect(),
            inputs: AtomicCell::new(self.inputs.load()),
        }
    }
}

/// Tracks whether any input read during a query's execution has any accumulated values.
///
/// Knowning whether any input has accumulated values makes aggregating the accumulated values
/// cheaper because we can skip over entire subtrees.
#[derive(Copy, Clone, Debug, Default)]
pub enum InputAccumulatedValues {
    /// The query nor any of its inputs have any accumulated values.
    #[default]
    Empty,

    /// The query or any of its inputs have at least one accumulated value.
    Any,
}

impl InputAccumulatedValues {
    pub(crate) fn from_map(accumulated: &AccumulatedMap) -> Self {
        if accumulated.map.is_empty() {
            accumulated.inputs.load()
        } else {
            Self::Any
        }
    }

    pub(crate) const fn is_any(self) -> bool {
        matches!(self, Self::Any)
    }

    pub(crate) const fn is_empty(self) -> bool {
        matches!(self, Self::Empty)
    }
}

impl BitOr for InputAccumulatedValues {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        if rhs.is_any() {
            InputAccumulatedValues::Any
        } else {
            self
        }
    }
}

impl BitOrAssign for InputAccumulatedValues {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}
