#![cfg(feature = "inventory")]

//! Test that the `Update` derive works as expected

#[derive(salsa::Update)]
struct MyInput {
    field: &'static str,
}

#[derive(salsa::Update)]
struct MyInput2 {
    #[update(unsafe(with(custom_update)))]
    field: &'static str,
    #[update(unsafe(with(|dest, data| { *dest = data; true })))]
    field2: &'static str,
}

#[derive(Debug, PartialEq)]
struct FallbackValue(&'static str);

#[derive(salsa::Update)]
struct MyInput3 {
    #[update(fallback)]
    field: FallbackValue,
}

unsafe fn custom_update(dest: *mut &'static str, _data: &'static str) -> bool {
    unsafe { *dest = "ill-behaved for testing purposes" };
    true
}

#[test]
fn derived() {
    let mut m = MyInput { field: "foo" };
    assert_eq!(m.field, "foo");
    assert!(unsafe { salsa::Update::maybe_update(&mut m, MyInput { field: "bar" }) });
    assert_eq!(m.field, "bar");
    assert!(!unsafe { salsa::Update::maybe_update(&mut m, MyInput { field: "bar" }) });
    assert_eq!(m.field, "bar");
}

#[test]
fn derived_fallback() {
    let mut m = MyInput3 {
        field: FallbackValue("foo"),
    };
    assert_eq!(m.field, FallbackValue("foo"));
    assert!(unsafe {
        salsa::Update::maybe_update(
            &mut m,
            MyInput3 {
                field: FallbackValue("bar"),
            },
        )
    });
    assert_eq!(m.field, FallbackValue("bar"));
    assert!(!unsafe {
        salsa::Update::maybe_update(
            &mut m,
            MyInput3 {
                field: FallbackValue("bar"),
            },
        )
    });
    assert_eq!(m.field, FallbackValue("bar"));
}

#[test]
fn derived_with() {
    let mut m = MyInput2 {
        field: "foo",
        field2: "foo",
    };
    assert_eq!(m.field, "foo");
    assert_eq!(m.field2, "foo");
    assert!(unsafe {
        salsa::Update::maybe_update(
            &mut m,
            MyInput2 {
                field: "bar",
                field2: "bar",
            },
        )
    });
    assert_eq!(m.field, "ill-behaved for testing purposes");
    assert_eq!(m.field2, "bar");
    assert!(unsafe {
        salsa::Update::maybe_update(
            &mut m,
            MyInput2 {
                field: "ill-behaved for testing purposes",
                field2: "foo",
            },
        )
    });
    assert_eq!(m.field, "ill-behaved for testing purposes");
    assert_eq!(m.field2, "foo");
}
