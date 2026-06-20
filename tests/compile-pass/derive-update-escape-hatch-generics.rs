#![allow(dead_code)]

struct NotUpdate;

#[derive(salsa::Update)]
struct Escaped<T> {
    #[update(unsafe(with(skip_update::<T>)))]
    value: T,
}

#[derive(salsa::Update)]
struct Mixed<T> {
    value: T,
    #[update(unsafe(with(skip_update::<T>)))]
    value2: T,
}

unsafe fn skip_update<T>(_: *mut T, _: T) -> bool {
    false
}

fn assert_update<T: salsa::Update>() {}

fn main() {
    assert_update::<Escaped<NotUpdate>>();
}
