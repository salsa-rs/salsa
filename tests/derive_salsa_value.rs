#[derive(salsa::SalsaValue)]
struct Generic<T>(T);

#[derive(salsa::SalsaValue)]
struct GenericPair<K, V>(K, V);

#[derive(salsa::SalsaValue)]
struct Bounded<T: Clone>(T);

#[derive(salsa::SalsaValue)]
struct GenericWithMarker<I, T> {
    value: T,
    #[salsa_value(prove_safe_to_retain_manually)]
    marker: std::marker::PhantomData<I>,
}

#[derive(salsa::SalsaValue)]
struct StaticValue;

#[derive(salsa::SalsaValue)]
struct ContainsStaticValue(StaticValue);

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
    type Output = Invariant<T::Output>;
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
    ContainsPhantomRef<'static>: salsa::SalsaValue<'db, Output = ContainsPhantomRef<'db>>,
{
}

fn assert_generic_output<'db>()
where
    GenericPair<String, ContainsPhantomRef<'static>>:
        salsa::SalsaValue<'db, Output = GenericPair<String, ContainsPhantomRef<'db>>>,
    GenericWithMarker<String, ContainsPhantomRef<'static>>:
        salsa::SalsaValue<'db, Output = GenericWithMarker<String, ContainsPhantomRef<'db>>>,
{
}

fn assert_invariant_container_output<'db>()
where
    ContainsInvariant<'static>: salsa::SalsaValue<'db, Output = ContainsInvariant<'db>>,
{
}

#[test]
fn derives_salsa_value() {
    let contains_phantom_ref = ContainsPhantomRef {
        marker: std::marker::PhantomData,
    };
    let generic_with_marker = GenericWithMarker {
        value: String::new(),
        marker: std::marker::PhantomData::<String>,
    };
    let contains_invariant = ContainsInvariant {
        value: Invariant::<ContainsPhantomRef<'static>>(std::marker::PhantomData),
    };
    let _ = (generic_with_marker.value, generic_with_marker.marker);
    let _ = contains_invariant.value.0;
    assert_contains_phantom_ref(contains_phantom_ref.marker);
    assert_generic_output();
    assert_invariant_container_output();
    let recursive = Recursive::Cons(Box::new(Recursive::Nil));
    let Recursive::Cons(recursive) = recursive else {
        unreachable!()
    };
    let _ = recursive;

    assert_salsa_value::<Generic<String>>();
    assert_salsa_value::<Bounded<String>>();
    assert_salsa_value::<ContainsPhantomRef<'static>>();
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
