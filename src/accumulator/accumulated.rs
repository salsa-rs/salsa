use std::fmt::Debug;

use crate::accumulator::Accumulator;

#[derive(Clone, Debug)]
pub(crate) struct Accumulated<A: Accumulator> {
    values: Vec<A>,
}

impl<A: Accumulator> Accumulated<A> {
    pub fn push(&mut self, value: A) {
        self.values.push(value);
    }

    pub fn extend_with_accumulated<'slf>(&'slf self, values: &mut Vec<&'slf A>) {
        values.extend(&self.values);
    }
}

impl<A: Accumulator> Default for Accumulated<A> {
    fn default() -> Self {
        Self {
            values: Default::default(),
        }
    }
}
