//! This examples shows how the `returns` attribute works for functions and structs.
//!
//! You can play with different `return` annotations to see how they change the
//! types and the clone/deref function calls..

/// Number wraps an i32 and is Copy.
#[derive(PartialEq, Eq, Copy, Debug)]
struct Number(i32);

// Dummy clone implementation that logs the Clone::clone call.
impl Clone for Number {
    #[allow(clippy::non_canonical_clone_impl)]
    fn clone(&self) -> Self {
        println!("Cloning {self:?}...");
        Number(self.0)
    }
}

// Deref into the wrapped i32 and log the call.
impl std::ops::Deref for Number {
    type Target = i32;

    fn deref(&self) -> &Self::Target {
        println!("Dereferencing {self:?}...");
        &self.0
    }
}

// Salsa struct.
#[salsa::input]
struct Input {
    // #[returns(ref)]
    // #[returns(deref)]
    // #[returns(copy)]
    #[returns(clone)]
    number: Number,
}

/// Salsa database to use in our example.
#[salsa::db]
#[derive(Clone, Default)]
struct NumDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for NumDb {}

// #[salsa::tracked(returns(clone))]
// #[salsa::tracked(returns(ref))]
// #[salsa::tracked(returns(copy))]
#[salsa::tracked(returns(deref))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}

fn main() {
    let db: NumDb = Default::default();
    let input = Input::new(&db, Number(42));

    // Call the salsa::tracked number function.
    let n = number(&db, input);
    eprintln!("n: {n:?}");
}
