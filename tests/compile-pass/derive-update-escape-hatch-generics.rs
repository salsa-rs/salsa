#![allow(dead_code)]

#[derive(PartialEq)]
struct NotUpdate;

#[derive(Clone)]
struct CloneOnly;

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

#[derive(salsa::Update)]
struct BoundEscaped<T, U> {
    #[update(bounds(T: 'static + PartialEq), unsafe(with(salsa::update_fallback::<T>)))]
    value: T,
    #[update(bounds(U: Clone, U: 'static), unsafe(with(update_with_clone_bound::<U>)))]
    value2: U,
}

unsafe fn skip_update<T>(_: *mut T, _: T) -> bool {
    false
}

unsafe fn update_with_clone_bound<T>(_: *mut T, _: T) -> bool
where
    T: Clone + 'static,
{
    false
}

fn assert_update<T: salsa::Update>() {}

fn main() {
    assert_update::<Escaped<NotUpdate>>();
    assert_update::<BoundEscaped<NotUpdate, CloneOnly>>();
}
