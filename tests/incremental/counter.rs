use crossbeam::atomic::AtomicCell;

#[derive(Default)]
pub(crate) struct Counter {
    value: AtomicCell<usize>,
}

impl Counter {
    pub(crate) fn increment(&self) -> usize {
        self.value.fetch_add(1)
    }
}
