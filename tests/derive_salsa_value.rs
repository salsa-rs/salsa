#[derive(salsa::SalsaValue)]
struct Generic<T>(T);

#[derive(salsa::SalsaValue)]
struct StaticValue;

#[derive(salsa::SalsaValue)]
struct ContainsStaticValue(StaticValue);

#[derive(salsa::SalsaValue)]
struct ContainsPhantomRef<'db> {
    marker: std::marker::PhantomData<&'db ()>,
}

#[derive(salsa::SalsaValue)]
enum Recursive {
    Nil,
    Cons(Box<Self>),
}

fn assert_salsa_value<T: salsa::SalsaValue>() {}

fn assert_contains_phantom_ref<'db>(_marker: std::marker::PhantomData<&'db ()>) {
    assert_salsa_value::<ContainsPhantomRef<'db>>();
}

#[test]
fn derives_salsa_value() {
    let contains_phantom_ref = ContainsPhantomRef {
        marker: std::marker::PhantomData,
    };
    assert_contains_phantom_ref(contains_phantom_ref.marker);
    let recursive = Recursive::Cons(Box::new(Recursive::Nil));
    let Recursive::Cons(recursive) = recursive else {
        unreachable!()
    };
    let _ = recursive;

    assert_salsa_value::<Generic<String>>();
    assert_salsa_value::<ContainsStaticValue>();
    assert_salsa_value::<Recursive>();
    assert_salsa_value::<std::num::NonZeroU32>();
    assert_salsa_value::<std::ops::Range<u32>>();
    assert_salsa_value::<std::ops::RangeInclusive<u32>>();
    assert_salsa_value::<std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>>(
    );
    assert_salsa_value::<
        std::collections::HashMap<
            String,
            String,
            std::hash::BuildHasherDefault<std::collections::hash_map::DefaultHasher>,
        >,
    >();
    assert_salsa_value::<salsa::Id>();
}
