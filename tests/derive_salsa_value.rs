#[derive(salsa::SalsaValue)]
struct StaticValue;

#[derive(salsa::SalsaValue)]
struct ContainsStaticValue(StaticValue);

#[derive(salsa::SalsaValue)]
struct ContainsStaticPhantom(std::marker::PhantomData<&'static str>);

#[derive(salsa::SalsaValue)]
struct Generic<T>(T);

#[derive(salsa::SalsaValue)]
struct GenericPhantom<T>(std::marker::PhantomData<T>);

#[derive(salsa::SalsaValue)]
struct ConstGeneric<const N: usize>([u8; N]);

#[derive(salsa::SalsaValue)]
struct ContainsPhantomRef<'db> {
    marker: std::marker::PhantomData<&'db ()>,
}

struct Invariant<T>(std::marker::PhantomData<fn(T) -> T>);

// SAFETY: `Invariant` stores no values and only transfers the lifetime
// relationship guaranteed by `T`.
unsafe impl<T> salsa::SalsaValue for Invariant<T> where T: salsa::SalsaValue {}

#[derive(salsa::SalsaValue)]
struct ContainsInvariant<'db> {
    value: Invariant<ContainsPhantomRef<'db>>,
}

#[derive(salsa::SalsaValue)]
enum Recursive {
    Nil,
    Cons(Box<Self>),
}

#[derive(salsa::SalsaValue)]
enum GenericRecursive<T> {
    Nil,
    Cons(T, Box<Self>),
}

#[derive(salsa::SalsaValue)]
struct LifetimeBuildHasher<'db>(std::marker::PhantomData<&'db ()>);

impl std::hash::BuildHasher for LifetimeBuildHasher<'_> {
    type Hasher = std::collections::hash_map::DefaultHasher;

    fn build_hasher(&self) -> Self::Hasher {
        std::collections::hash_map::DefaultHasher::new()
    }
}

fn assert_salsa_value<T: salsa::SalsaValue>() {}

fn assert_contains_phantom_ref<'db>(_marker: std::marker::PhantomData<&'db ()>)
where
    ContainsPhantomRef<'db>: salsa::SalsaValue,
{
}

fn assert_invariant_container_output<'db>()
where
    ContainsInvariant<'db>: salsa::SalsaValue,
{
}

fn assert_generic_phantom<'db>()
where
    GenericPhantom<&'db ()>: salsa::SalsaValue,
{
}

fn assert_non_static_standard_values<'db>()
where
    std::collections::HashMap<String, String, LifetimeBuildHasher<'db>>: salsa::SalsaValue,
    std::collections::HashSet<String, LifetimeBuildHasher<'db>>: salsa::SalsaValue,
    std::hash::BuildHasherDefault<&'db ()>: salsa::SalsaValue,
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
    let recursive = GenericRecursive::Cons(String::new(), Box::new(GenericRecursive::Nil));
    let GenericRecursive::Cons(value, recursive) = recursive else {
        unreachable!()
    };
    let _ = (value, recursive);

    assert_generic_phantom();
    assert_non_static_standard_values();
    assert_salsa_value::<Generic<String>>();
    assert_salsa_value::<Generic<ContainsPhantomRef<'static>>>();
    assert_salsa_value::<GenericRecursive<String>>();
    assert_salsa_value::<ConstGeneric<4>>();
    assert_salsa_value::<ContainsPhantomRef<'static>>();
    assert_salsa_value::<ContainsStaticPhantom>();
    assert_salsa_value::<ContainsStaticValue>();
    assert_salsa_value::<Recursive>();
    assert_salsa_value::<Box<str>>();
    assert_salsa_value::<Box<std::path::Path>>();
    assert_salsa_value::<Box<[u32]>>();
    assert_salsa_value::<std::rc::Rc<str>>();
    assert_salsa_value::<std::sync::Arc<str>>();
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
