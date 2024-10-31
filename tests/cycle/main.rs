//! Test cases for fixpoint iteration cycle resolution.
//!
//! These test cases use a generic query setup that allows constructing arbitrary dependency
//! graphs, and attempts to achieve good coverage of various cases.
mod dataflow;

use salsa::{CycleRecoveryAction, Database as Db, DatabaseImpl as DbImpl, Durability, Setter};

/// A vector of inputs a query can evaluate to get an iterator of u8 values to operate on.
///
/// This allows creating arbitrary query graphs between the four queries below (`min_iterate`,
/// `max_iterate`, `min_panic`, `max_panic`) for testing cycle behaviors.
#[salsa::input]
struct Inputs {
    inputs: Vec<Input>,
}

impl Inputs {
    fn values(self, db: &dyn Db) -> impl Iterator<Item = u8> + '_ {
        self.inputs(db).into_iter().map(|input| input.eval(db))
    }
}

/// A single input, evaluating to a single u8 value.
#[derive(Clone, Debug)]
enum Input {
    /// a simple value
    Value(u8),

    /// a simple value, reported as an untracked read
    UntrackedRead(u8),

    /// minimum of the given inputs, with fixpoint iteration on cycles
    MinIterate(Inputs),

    /// maximum of the given inputs, with fixpoint iteration on cycles
    MaxIterate(Inputs),

    /// minimum of the given inputs, panicking on cycles
    MinPanic(Inputs),

    /// maximum of the given inputs, panicking on cycles
    MaxPanic(Inputs),

    /// value of the given input, plus one
    Successor(Box<Input>),
}

impl Input {
    fn eval(self, db: &dyn Db) -> u8 {
        match self {
            Self::Value(value) => value,
            Self::UntrackedRead(value) => {
                db.report_untracked_read();
                value
            }
            Self::MinIterate(inputs) => min_iterate(db, inputs),
            Self::MaxIterate(inputs) => max_iterate(db, inputs),
            Self::MinPanic(inputs) => min_panic(db, inputs),
            Self::MaxPanic(inputs) => max_panic(db, inputs),
            Self::Successor(input) => input.eval(db) + 1,
        }
    }

    fn assert(self, db: &dyn Db, expected: u8) {
        assert_eq!(self.eval(db), expected)
    }
}

#[salsa::tracked(cycle_fn=min_recover, cycle_initial=min_initial)]
fn min_iterate<'db>(db: &'db dyn Db, inputs: Inputs) -> u8 {
    inputs.values(db).min().expect("empty inputs!")
}

const MIN_COUNT_FALLBACK: u8 = 100;
const MIN_VALUE_FALLBACK: u8 = 5;
const MIN_VALUE: u8 = 10;

fn min_recover(_db: &dyn Db, value: &u8, count: u32) -> CycleRecoveryAction<u8> {
    if *value < MIN_VALUE {
        CycleRecoveryAction::Fallback(MIN_VALUE_FALLBACK)
    } else if count > 10 {
        CycleRecoveryAction::Fallback(MIN_COUNT_FALLBACK)
    } else {
        CycleRecoveryAction::Iterate
    }
}

fn min_initial(_db: &dyn Db) -> u8 {
    255
}

#[salsa::tracked(cycle_fn=max_recover, cycle_initial=max_initial)]
fn max_iterate<'db>(db: &'db dyn Db, inputs: Inputs) -> u8 {
    inputs.values(db).max().expect("empty inputs!")
}

const MAX_COUNT_FALLBACK: u8 = 200;
const MAX_VALUE_FALLBACK: u8 = 250;
const MAX_VALUE: u8 = 245;

fn max_recover(_db: &dyn Db, value: &u8, count: u32) -> CycleRecoveryAction<u8> {
    if *value > MAX_VALUE {
        CycleRecoveryAction::Fallback(MAX_VALUE_FALLBACK)
    } else if count > 10 {
        CycleRecoveryAction::Fallback(MAX_COUNT_FALLBACK)
    } else {
        CycleRecoveryAction::Iterate
    }
}

fn max_initial(_db: &dyn Db) -> u8 {
    0
}

#[salsa::tracked]
fn min_panic<'db>(db: &'db dyn Db, inputs: Inputs) -> u8 {
    inputs.values(db).min().expect("empty inputs!")
}

#[salsa::tracked]
fn max_panic<'db>(db: &'db dyn Db, inputs: Inputs) -> u8 {
    inputs.values(db).max().expect("empty inputs!")
}

// Diagram nomenclature for nodes: Each node is represented as a:xx(ii), where `a` is a sequential
// identifier from `a`, `b`, `c`..., xx is one of the four query kinds:
// - `Ni` for `min_iterate`
// - `Xi` for `max_iterate`
// - `Np` for `min_panic`
// - `Xp` for `max_panic`
//
// and `ii` is the inputs for that query, represented as a comma-separated list, with each
// component representing an input:
// - `a`, `b`, `c`... where the input is another node,
// - `uXX` for `UntrackedRead(XX)`
// - `vXX` for `Value(XX)`
// - `sY` for `Successor(Y)`
//
// We always enter from the top left node in the diagram.

/// a:Np(a) -+
/// ^        |
/// +--------+
///
/// Simple self-cycle, no iteration, should panic.
#[test]
#[should_panic(expected = "dependency graph cycle")]
fn self_panic() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let a = Input::MinPanic(a_in);
    a_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.eval(&db);
}

/// a:Np(u10, a) -+
/// ^             |
/// +-------------+
///
/// Simple self-cycle with untracked read, no iteration, should panic.
#[test]
#[should_panic(expected = "dependency graph cycle")]
fn self_untracked_panic() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let a = Input::MinPanic(a_in);
    a_in.set_inputs(&mut db)
        .to(vec![Input::UntrackedRead(10), a.clone()]);

    a.eval(&db);
}

/// a:Ni(a) -+
/// ^        |
/// +--------+
///
/// Simple self-cycle, iteration converges on initial value.
#[test]
fn self_converge_initial_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    a_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.assert(&db, 255);
}

/// a:Ni(b) --> b:Np(a)
/// ^                 |
/// +-----------------+
///
/// Two-query cycle, one with iteration and one without.
/// If we enter from the one with iteration, we converge on its initial value.
#[test]
fn two_mixed_converge_initial_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinPanic(b_in);
    a_in.set_inputs(&mut db).to(vec![b]);
    b_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.assert(&db, 255);
}

/// a:Np(b) --> b:Ni(a)
/// ^                 |
/// +-----------------+
///
/// Two-query cycle, one with iteration and one without.
/// If we enter from the one with no iteration, we panic.
#[test]
#[should_panic(expected = "dependency graph cycle")]
fn two_mixed_panic() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let a = Input::MinPanic(b_in);
    let b = Input::MinIterate(a_in);
    a_in.set_inputs(&mut db).to(vec![b]);
    b_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.eval(&db);
}

/// a:Ni(b) --> b:Xi(a)
/// ^                 |
/// +-----------------+
///
/// Two-query cycle, both with iteration.
/// We converge on the initial value of whichever we first enter from.
#[test]
fn two_iterate_converge_initial_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MaxIterate(b_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.assert(&db, 255);
    b.assert(&db, 255);
}

/// a:Xi(b) --> b:Ni(a)
/// ^                 |
/// +-----------------+
///
/// Two-query cycle, both with iteration.
/// We converge on the initial value of whichever we enter from.
/// (Same setup as above test, different query order.)
#[test]
fn two_iterate_converge_initial_value_2() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MinIterate(b_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.assert(&db, 0);
    b.assert(&db, 0);
}

/// a:Np(b) --> b:Ni(c) --> c:Xp(b)
///             ^                 |
///             +-----------------+
///
/// Two-query cycle, enter indirectly at node with iteration, converge on its initial value.
#[test]
fn two_indirect_iterate_converge_initial_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinPanic(a_in);
    let b = Input::MinIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db).to(vec![b]);

    a.assert(&db, 255);
}

/// a:Xp(b) --> b:Np(c) --> c:Xi(b)
///             ^                 |
///             +-----------------+
///
/// Two-query cycle, enter indirectly at node without iteration, panic.
#[test]
#[should_panic(expected = "dependency graph cycle")]
fn two_indirect_panic() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinPanic(a_in);
    let b = Input::MinPanic(b_in);
    let c = Input::MaxIterate(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db).to(vec![b]);

    a.eval(&db);
}

/// a:Np(b) -> b:Ni(v250,c) -> c:Xp(b)
///            ^                     |
///            +---------------------+
///
/// Two-query cycle, converges to non-initial value.
#[test]
fn two_converge() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinPanic(a_in);
    let b = Input::MinIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![Input::Value(250), c]);
    c_in.set_inputs(&mut db).to(vec![b]);

    a.assert(&db, 250);
}

/// a:Xp(b) -> b:Xi(v10,c) -> c:Xp(sb)
///            ^                     |
///            +---------------------+
///
/// Two-query cycle, falls back due to >10 iterations.
#[test]
fn two_fallback_count() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxPanic(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![Input::Value(10), c]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Successor(Box::new(b))]);

    a.assert(&db, MAX_COUNT_FALLBACK + 1);
}

/// a:Xp(b) -> b:Xi(v241,c) -> c:Xp(sb)
///            ^                     |
///            +---------------------+
///
/// Two-query cycle, falls back due to value reaching >MAX_VALUE (we start at 241 and each
/// iteration increments until we reach >245).
#[test]
fn two_fallback_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxPanic(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![Input::Value(241), c]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Successor(Box::new(b))]);

    a.assert(&db, MAX_VALUE_FALLBACK + 1);
}

/// a:Ni(b) -> b:Np(a, c) -> c:Np(v25, a)
/// ^          |                        |
/// +----------+------------------------+
///
/// Three-query cycle, (b) and (c) both depend on (a). We converge on 25.
#[test]
fn three_fork_converge() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinPanic(b_in);
    let c = Input::MinPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b]);
    b_in.set_inputs(&mut db).to(vec![a.clone(), c]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Value(25), a.clone()]);

    a.assert(&db, 25);
}

/// a:Ni(b) -> b:Ni(a, c) -> c:Np(v25, b)
/// ^          |        ^          |
/// +----------+        +----------+
///
/// Layered cycles. We converge on 25.
#[test]
fn layered_converge() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinIterate(b_in);
    let c = Input::MinPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![a.clone(), c]);
    c_in.set_inputs(&mut db).to(vec![Input::Value(25), b]);

    a.assert(&db, 25);
}

/// a:Xi(b) -> b:Xi(a, c) -> c:Xp(v25, sb)
/// ^          |        ^          |
/// +----------+        +----------+
///
/// Layered cycles. We hit max iterations and fall back.
#[test]
fn layered_fallback_count() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![a.clone(), c]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Value(25), Input::Successor(Box::new(b))]);

    a.assert(&db, MAX_COUNT_FALLBACK + 1);
}

/// a:Xi(b) -> b:Xi(a, c) -> c:Xp(v240, sb)
/// ^          |        ^          |
/// +----------+        +----------+
///
/// Layered cycles. We hit max value and fall back.
#[test]
fn layered_fallback_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![a.clone(), c]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Value(240), Input::Successor(Box::new(b))]);

    a.assert(&db, MAX_VALUE_FALLBACK + 1);
}

/// a:Ni(b) -> b:Ni(c) -> c:Np(v25, a, b)
/// ^          ^                        |
/// +----------+------------------------+
///
/// Nested cycles. We converge on 25.
#[test]
fn nested_converge() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinIterate(b_in);
    let c = Input::MinPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Value(25), a.clone(), b]);

    a.assert(&db, 25);
}

/// a:Ni(b) -> b:Ni(c) -> c:Np(v25, b, a)
/// ^          ^                        |
/// +----------+------------------------+
///
/// Nested cycles, inner first. We converge on 25.
#[test]
fn nested_inner_first_converge() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinIterate(b_in);
    let c = Input::MinPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Value(25), b, a.clone()]);

    a.assert(&db, 25);
}

/// a:Xi(b) -> b:Xi(c) -> c:Xp(v25, a, sb)
/// ^          ^                         |
/// +----------+-------------------------+
///
/// Nested cycles. We hit max iterations and fall back.
#[test]
fn nested_fallback_count() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db).to(vec![
        Input::Value(25),
        a.clone(),
        Input::Successor(Box::new(b)),
    ]);

    a.assert(&db, MAX_COUNT_FALLBACK + 1);
}

/// a:Xi(b) -> b:Xi(c) -> c:Xp(v25, b, sa)
/// ^          ^                         |
/// +----------+-------------------------+
///
/// Nested cycles, inner first. We hit max iterations and fall back.
#[test]
fn nested_inner_first_fallback_count() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db).to(vec![
        Input::Value(25),
        b,
        Input::Successor(Box::new(a.clone())),
    ]);

    a.assert(&db, MAX_COUNT_FALLBACK + 1);
}

/// a:Xi(b) -> b:Xi(c) -> c:Xp(v240, a, sb)
/// ^          ^                          |
/// +----------+--------------------------+
///
/// Nested cycles. We hit max value and fall back.
#[test]
fn nested_fallback_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db).to(vec![
        Input::Value(240),
        a.clone(),
        Input::Successor(Box::new(b)),
    ]);

    a.assert(&db, MAX_VALUE_FALLBACK + 1);
}

/// a:Xi(b) -> b:Xi(c) -> c:Xp(v240, b, sa)
/// ^          ^                          |
/// +----------+--------------------------+
///
/// Nested cycles, inner first. We hit max value and fall back.
#[test]
fn nested_inner_first_fallback_value() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c]);
    c_in.set_inputs(&mut db).to(vec![
        Input::Value(240),
        b,
        Input::Successor(Box::new(a.clone())),
    ]);

    a.assert(&db, MAX_VALUE_FALLBACK + 1);
}

/// a:Ni(b) -> b:Ni(c, a) -> c:Np(v25, a, b)
/// ^          ^        |                  |
/// +----------+--------|------------------+
/// |                   |
/// +-------------------+
///
/// Nested cycles, double head. We converge on 25.
#[test_log::test]
fn nested_double_converge() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinIterate(b_in);
    let c = Input::MinPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c, a.clone()]);
    c_in.set_inputs(&mut db)
        .to(vec![Input::Value(25), a.clone(), b]);

    a.assert(&db, 25);
}

// Multiple-revision cycles

/// a:Ni(b) --> b:Np(a)
/// ^                 |
/// +-----------------+
///
/// a:Ni(b) --> b:Np(v30)
///
/// Cycle becomes not-a-cycle in next revision.
#[test]
fn cycle_becomes_non_cycle() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinPanic(b_in);
    a_in.set_inputs(&mut db).to(vec![b]);
    b_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.clone().assert(&db, 255);

    b_in.set_inputs(&mut db).to(vec![Input::Value(30)]);

    a.assert(&db, 30);
}

/// a:Ni(b) --> b:Np(v30)
///
/// a:Ni(b) --> b:Np(a)
/// ^                 |
/// +-----------------+
///
/// Non-cycle becomes a cycle in next revision.
#[test]
fn non_cycle_becomes_cycle() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinPanic(b_in);
    a_in.set_inputs(&mut db).to(vec![b]);
    b_in.set_inputs(&mut db).to(vec![Input::Value(30)]);

    a.clone().assert(&db, 30);

    b_in.set_inputs(&mut db).to(vec![a.clone()]);

    a.assert(&db, 255);
}

/// a:Xi(b) -> b:Xi(c, a) -> c:Xp(v25, a, sb)
/// ^          ^        |                   |
/// +----------+--------|-------------------+
/// |                   |
/// +-------------------+
///
/// Nested cycles, double head. We hit max iterations and fall back, then max value on the next
/// revision, then converge on the next.
#[test]
fn nested_double_multiple_revisions() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MaxIterate(a_in);
    let b = Input::MaxIterate(b_in);
    let c = Input::MaxPanic(c_in);
    a_in.set_inputs(&mut db).to(vec![b.clone()]);
    b_in.set_inputs(&mut db).to(vec![c, a.clone()]);
    c_in.set_inputs(&mut db).to(vec![
        Input::Value(25),
        a.clone(),
        Input::Successor(Box::new(b.clone())),
    ]);

    a.clone().assert(&db, MAX_COUNT_FALLBACK + 1);

    // next revision, we hit max value instead
    c_in.set_inputs(&mut db).to(vec![
        Input::Value(240),
        a.clone(),
        Input::Successor(Box::new(b.clone())),
    ]);

    a.clone().assert(&db, MAX_VALUE_FALLBACK + 1);

    // and next revision, we converge
    c_in.set_inputs(&mut db)
        .to(vec![Input::Value(240), a.clone(), b]);

    a.assert(&db, 240);
}

/// a:Ni(b) -> b:Ni(c) -> c:Ni(a)
/// ^                           |
/// +---------------------------+
///
/// In a cycle with some LOW durability and some HIGH durability inputs, changing a LOW durability
/// input still re-executes the full cycle in the next revision.
#[test]
fn cycle_durability() {
    let mut db = DbImpl::new();
    let a_in = Inputs::new(&db, vec![]);
    let b_in = Inputs::new(&db, vec![]);
    let c_in = Inputs::new(&db, vec![]);
    let a = Input::MinIterate(a_in);
    let b = Input::MinIterate(b_in);
    let c = Input::MinIterate(c_in);
    a_in.set_inputs(&mut db)
        .with_durability(Durability::LOW)
        .to(vec![b.clone()]);
    b_in.set_inputs(&mut db)
        .with_durability(Durability::HIGH)
        .to(vec![c]);
    c_in.set_inputs(&mut db)
        .with_durability(Durability::HIGH)
        .to(vec![a.clone()]);

    a.clone().assert(&db, 255);

    // next revision, we converge instead
    a_in.set_inputs(&mut db)
        .with_durability(Durability::LOW)
        .to(vec![Input::Value(45), b]);

    a.assert(&db, 45);
}
