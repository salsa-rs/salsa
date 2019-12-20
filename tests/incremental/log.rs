use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct Log {
    data: Mutex<Vec<String>>,
}

impl Log {
    pub(crate) fn add(&self, text: impl Into<String>) {
        self.data.lock().unwrap().push(text.into());
    }

    pub(crate) fn take(&self) -> Vec<String> {
        std::mem::replace(&mut *self.data.lock().unwrap(), vec![])
    }
}
