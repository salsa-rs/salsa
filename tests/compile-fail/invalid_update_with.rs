#[derive(salsa::Update)]
struct S2 {
    #[update(unsafe(with(my_wrong_update)))]
    bad: i32,
    #[update(unsafe(with(my_wrong_update2)))]
    bad2: i32,
    #[update(unsafe(with(my_wrong_update3)))]
    bad3: i32,
    #[update(unsafe(with(true)))]
    bad4: &'static str,
}

fn my_wrong_update() {}
fn my_wrong_update2(_: (), _: ()) -> bool {
    true
}
fn my_wrong_update3(_: *mut i32, _: i32) -> () {}

fn main() {}
