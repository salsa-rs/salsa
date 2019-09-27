//! Test that "on-demand" input pattern works.
//!
//! On-demand inputs are inputs computed lazily on the fly. They are simulated
//! via a b query with zero inputs, which uses `add_synthetic_read` to
//! tweak durability and `invalidate` to clear the input.

use std::{cell::Cell, collections::HashMap, rc::Rc};

use salsa::{Database as _, Durability};

#[salsa::query_group(QueryGroupStorage)]
trait QueryGroup: salsa::Database + AsRef<HashMap<u32, u32>> {
    fn a(&self, x: u32) -> u32;
    fn b(&self, x: u32) -> u32;
    fn c(&self, x: u32) -> u32;
}

fn a(db: &impl QueryGroup, x: u32) -> u32 {
    let durability = if x % 2 == 0 {
        Durability::LOW
    } else {
        Durability::HIGH
    };
    db.salsa_runtime().report_synthetic_read(durability);
    let external_state: &HashMap<u32, u32> = db.as_ref();
    external_state[&x]
}

fn b(db: &impl QueryGroup, x: u32) -> u32 {
    db.a(x)
}

fn c(db: &impl QueryGroup, x: u32) -> u32 {
    db.b(x)
}

#[salsa::database(QueryGroupStorage)]
#[derive(Default)]
struct Database {
    runtime: salsa::Runtime<Database>,
    external_state: HashMap<u32, u32>,
    on_event: Option<Box<dyn Fn(salsa::Event<Database>)>>,
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }

    fn salsa_event(&self, event_fn: impl Fn() -> salsa::Event<Self>) {
        if let Some(cb) = &self.on_event {
            cb(event_fn())
        }
    }
}

impl AsRef<HashMap<u32, u32>> for Database {
    fn as_ref(&self) -> &HashMap<u32, u32> {
        &self.external_state
    }
}

#[test]
fn on_demand_input_works() {
    let mut db = Database::default();

    db.external_state.insert(1, 10);
    assert_eq!(db.b(1), 10);
    assert_eq!(db.a(1), 10);

    // We changed external state, but haven't signaled about this yet,
    // so we expect to see the old answer
    db.external_state.insert(1, 92);
    assert_eq!(db.b(1), 10);
    assert_eq!(db.a(1), 10);

    db.query_mut(AQuery).invalidate(&1);
    assert_eq!(db.b(1), 92);
    assert_eq!(db.a(1), 92);
}

#[test]
fn on_demand_input_durability() {
    let mut db = Database::default();
    db.external_state.insert(1, 10);
    db.external_state.insert(2, 20);
    assert_eq!(db.b(1), 10);
    assert_eq!(db.b(2), 20);

    let validated = Rc::new(Cell::new(0));
    db.on_event = Some(Box::new({
        let validated = Rc::clone(&validated);
        move |event| match event.kind {
            salsa::EventKind::DidValidateMemoizedValue { .. } => validated.set(validated.get() + 1),
            _ => (),
        }
    }));

    db.salsa_runtime_mut().synthetic_write(Durability::LOW);
    validated.set(0);
    assert_eq!(db.c(1), 10);
    assert_eq!(db.c(2), 20);
    assert_eq!(validated.get(), 2);

    db.salsa_runtime_mut().synthetic_write(Durability::HIGH);
    validated.set(0);
    assert_eq!(db.c(1), 10);
    assert_eq!(db.c(2), 20);
    assert_eq!(validated.get(), 4);
}
