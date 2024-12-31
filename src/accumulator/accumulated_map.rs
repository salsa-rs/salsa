use std::ops;

use rustc_hash::FxHashMap;

use crate::IngredientIndex;

use super::{accumulated::Accumulated, Accumulator, AnyAccumulated};

#[derive(Default, Debug)]
pub struct AccumulatedMap {
    map: FxHashMap<IngredientIndex, Box<dyn AnyAccumulated>>,
}

impl AccumulatedMap {
    pub fn accumulate<A: Accumulator>(&mut self, index: IngredientIndex, value: A) {
        self.map
            .entry(index)
            .or_insert_with(|| <Box<Accumulated<A>>>::default())
            .accumulate(value);
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

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
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
    pub const fn is_any(self) -> bool {
        matches!(self, Self::Any)
    }

    pub const fn is_empty(self) -> bool {
        matches!(self, Self::Empty)
    }
}

impl ops::BitOr for InputAccumulatedValues {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Any, _) | (_, Self::Any) => Self::Any,
            (Self::Empty, Self::Empty) => Self::Empty,
        }
    }
}

impl ops::BitOrAssign for InputAccumulatedValues {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}
