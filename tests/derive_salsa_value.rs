#[derive(salsa::SalsaValue)]
struct StaticValue;

#[derive(salsa::SalsaValue)]
struct ContainsStaticValue(StaticValue);

#[derive(salsa::SalsaValue)]
struct ContainsStaticPhantom(std::marker::PhantomData<&'static str>);

#[derive(salsa::SalsaValue)]
struct ContainsPhantomRef<'db> {
    marker: std::marker::PhantomData<&'db ()>,
}

struct Invariant<T>(std::marker::PhantomData<fn(T) -> T>);

// SAFETY: `Invariant` stores no values and only transfers the lifetime
// relationship guaranteed by `T`.
unsafe impl<'db, T> salsa::SalsaValue<'db> for Invariant<T>
where
    T: salsa::SalsaValue<'db>,
{
    type WithDb = Invariant<T::WithDb>;
}

#[derive(salsa::SalsaValue)]
struct ContainsInvariant<'db> {
    value: Invariant<ContainsPhantomRef<'db>>,
}

#[derive(salsa::SalsaValue)]
enum Recursive {
    Nil,
    Cons(Box<Self>),
}

fn assert_salsa_value<T: for<'db> salsa::SalsaValue<'db>>() {}

fn assert_contains_phantom_ref<'db>(_marker: std::marker::PhantomData<&'db ()>)
where
    ContainsPhantomRef<'static>: salsa::SalsaValue<'db, WithDb = ContainsPhantomRef<'db>>,
{
}

fn assert_invariant_container_output<'db>()
where
    ContainsInvariant<'static>: salsa::SalsaValue<'db, WithDb = ContainsInvariant<'db>>,
{
}

#[test]
fn derives_salsa_value() {
    let contains_phantom_ref = ContainsPhantomRef {
        marker: std::marker::PhantomData,
    };
    let contains_invariant = ContainsInvariant {
        value: Invariant::<ContainsPhantomRef<'static>>(std::marker::PhantomData),
    };
    let _ = contains_invariant.value.0;
    assert_contains_phantom_ref(contains_phantom_ref.marker);
    assert_invariant_container_output();
    let recursive = Recursive::Cons(Box::new(Recursive::Nil));
    let Recursive::Cons(recursive) = recursive else {
        unreachable!()
    };
    let _ = recursive;

    assert_salsa_value::<ContainsPhantomRef<'static>>();
    assert_salsa_value::<ContainsStaticPhantom>();
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
