use std::cell::RefCell;

#[derive(Default)]
crate struct Log {
    data: RefCell<Vec<String>>,
}

impl Log {
    crate fn add(&self, text: impl Into<String>) {
        self.data.borrow_mut().push(text.into());
    }

    crate fn take(&self) -> Vec<String> {
        std::mem::replace(&mut *self.data.borrow_mut(), vec![])
    }
}
