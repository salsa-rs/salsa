use std::marker::PhantomData;
use std::rc::Rc;

use salsa::plumbing::{AsId, FromId};

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct LocalId {
    id: salsa::Id,
    not_send_or_sync: PhantomData<Rc<()>>,
}

impl AsId for LocalId {
    fn as_id(&self) -> salsa::Id {
        self.id
    }
}

impl FromId for LocalId {
    fn from_id(id: salsa::Id) -> Self {
        Self {
            id,
            not_send_or_sync: PhantomData,
        }
    }
}

#[salsa::interned(no_lifetime, id = LocalId)]
struct Interned {
    value: u32,
}

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

fn main() {
    assert_send::<Interned>();
    assert_sync::<Interned>();
}
