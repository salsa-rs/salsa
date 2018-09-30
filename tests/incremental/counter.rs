use std::cell::Cell;

#[derive(Default)]
crate struct Counter {
    value: Cell<usize>,
}

impl Counter {
    crate fn increment(&self) -> usize {
        let v = self.value.get();
        self.value.set(v + 1);
        v
    }
}
