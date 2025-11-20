#![cfg(feature = "inventory")]

#[salsa::interned(debug)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)]
enum InternedEnum<'db> {
    Unit,
    Tuple(u8, u8),
    Wrap(Box<Self>),
    Ref(&'db ()),
}

#[salsa::interned(debug, data = CustomPayload)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CustomDataEnum {
    One(u32),
    Two(String),
}

#[salsa::interned(no_lifetime, debug)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum NoLifetimeInterned {
    Item(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CollisionData<'db>(std::marker::PhantomData<&'db ()>);

// Field type intentionally matches the auto data name (`CollisionData`) to
// exercise the hygiene fallback for generated data identifiers.
#[salsa::interned(debug)]
struct Collision<'db> {
    payload: CollisionData<'db>,
}

#[test]
fn supports_enums() {
    let db = salsa::DatabaseImpl::new();

    let unit1 = InternedEnum::new(&db, InternedEnumData::Unit);
    let unit2 = InternedEnum::new(&db, InternedEnumData::Unit);
    assert_eq!(unit1, unit2);

    let wrapped = InternedEnum::new(
        &db,
        InternedEnumData::Wrap(Box::new(InternedEnumData::Tuple(1, 2))),
    );
    assert_eq!(
        wrapped.value(&db),
        InternedEnumData::Wrap(Box::new(InternedEnumData::Tuple(1, 2)))
    );
}

#[test]
fn respects_custom_data_name() {
    let db = salsa::DatabaseImpl::new();

    let v = CustomDataEnum::new(&db, CustomPayload::Two("hi".into()));
    assert_eq!(v.value(&db), CustomPayload::Two("hi".into()));

    let v2 = CustomDataEnum::new(&db, CustomPayload::One(1));
    assert_eq!(v2.value(&db), CustomPayload::One(1));
}

#[test]
fn supports_no_lifetime_enum() {
    let db = salsa::DatabaseImpl::new();

    let v = NoLifetimeInterned::new(&db, NoLifetimeInternedData::Item("static"));
    assert_eq!(v.value(&db), NoLifetimeInternedData::Item("static"));
}

#[test]
fn auto_data_name_conflict_is_renamed() {
    let db = salsa::DatabaseImpl::new();

    let collision = Collision::new(&db, CollisionData(std::marker::PhantomData));
    assert_eq!(
        collision.payload(&db),
        CollisionData(std::marker::PhantomData)
    );
}
