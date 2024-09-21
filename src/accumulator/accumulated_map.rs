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
