//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use std::any::TypeId;
use std::convert::identity;

use salsa::plumbing::Zalsa;
use test_log::test;

#[test]
fn interning_returns_equal_keys_for_equal_data() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedString::new(&db, "Hello, ".to_string(), identity);
    let s2 = InternedString::new(&db, "World, ".to_string(), |_| s1);
    let s1_2 = InternedString::new(&db, "Hello, ", identity);
    let s2_2 = InternedString::new(&db, "World, ", |_| s2);
    assert_eq!(s1, s1_2);
    assert_eq!(s2, s2_2);
}
// Recursive expansion of interned macro
// #[salsa::interned]
// struct InternedString<'db> {
//     data: String,
//     other: InternedString<'db>,
// }
// ======================================

#[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
struct InternedString<'db>(
    salsa::Id,
    std::marker::PhantomData<&'db salsa::plumbing::interned::Value<InternedString<'static>>>,
);

#[allow(warnings)]
const _: () = {
    use salsa::plumbing as zalsa_;
    use zalsa_::interned as zalsa_struct_;
    type Configuration_ = InternedString<'static>;
    #[derive(Clone)]
    struct StructData<'db>(String, InternedString<'db>);

    impl<'db> Eq for StructData<'db> {}
    impl<'db> PartialEq for StructData<'db> {
        fn eq(&self, other: &Self) -> bool {
            self.0 == other.0
        }
    }

    impl<'db> std::hash::Hash for StructData<'db> {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            self.0.hash(state);
        }
    }

    #[doc = r" Key to use during hash lookups. Each field is some type that implements `Lookup<T>`"]
    #[doc = r" for the owned type. This permits interning with an `&str` when a `String` is required and so forth."]
    #[derive(Hash)]
    struct StructKey<'db, T0>(T0, std::marker::PhantomData<&'db ()>);

    impl<'db, T0> zalsa_::interned::HashEqLike<StructKey<'db, T0>> for StructData<'db>
    where
        String: zalsa_::interned::HashEqLike<T0>,
    {
        fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
            zalsa_::interned::HashEqLike::<T0>::hash(&self.0, &mut *h);
        }
        fn eq(&self, data: &StructKey<'db, T0>) -> bool {
            (zalsa_::interned::HashEqLike::<T0>::eq(&self.0, &data.0) && true)
        }
    }
    impl zalsa_struct_::Configuration for Configuration_ {
        const DEBUG_NAME: &'static str = "InternedString";
        type Fields<'a> = StructData<'a>;
        type Struct<'a> = InternedString<'a>;
        fn struct_from_id<'db>(id: salsa::Id) -> Self::Struct<'db> {
            InternedString(id, std::marker::PhantomData)
        }
        fn deref_struct(s: Self::Struct<'_>) -> salsa::Id {
            s.0
        }
    }
    impl Configuration_ {
        pub fn ingredient<Db>(db: &Db) -> &zalsa_struct_::IngredientImpl<Self>
        where
            Db: ?Sized + zalsa_::Database,
        {
            static CACHE: zalsa_::IngredientCache<zalsa_struct_::IngredientImpl<Configuration_>> =
                zalsa_::IngredientCache::new();
            CACHE.get_or_create(db.as_dyn_database(), || {
                db.zalsa()
                    .add_or_lookup_jar_by_type::<zalsa_struct_::JarImpl<Configuration_>>()
            })
        }
    }
    impl zalsa_::AsId for InternedString<'_> {
        fn as_id(&self) -> salsa::Id {
            self.0
        }
    }
    impl zalsa_::FromId for InternedString<'_> {
        fn from_id(id: salsa::Id) -> Self {
            Self(id, std::marker::PhantomData)
        }
    }
    unsafe impl Send for InternedString<'_> {}

    unsafe impl Sync for InternedString<'_> {}

    impl std::fmt::Debug for InternedString<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            Self::default_debug_fmt(*self, f)
        }
    }
    impl zalsa_::SalsaStructInDb for InternedString<'_> {
        fn lookup_or_create_ingredient_index(aux: &Zalsa) -> salsa::plumbing::IngredientIndices {
            aux.add_or_lookup_jar_by_type::<zalsa_struct_::JarImpl<Configuration_>>()
                .into()
        }

        #[inline]
        fn cast(id: zalsa_::Id, type_id: TypeId) -> Option<Self> {
            if type_id == TypeId::of::<InternedString>() {
                Some(<InternedString as zalsa_::FromId>::from_id(id))
            } else {
                None
            }
        }
    }

    unsafe impl zalsa_::Update for InternedString<'_> {
        unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
            if unsafe { *old_pointer } != new_value {
                unsafe { *old_pointer = new_value };
                true
            } else {
                false
            }
        }
    }
    impl<'db> InternedString<'db> {
        pub fn new<Db_, T0: zalsa_::interned::Lookup<String> + std::hash::Hash>(
            db: &'db Db_,
            data: T0,
            other: impl FnOnce(InternedString<'db>) -> InternedString<'db>,
        ) -> Self
        where
            Db_: ?Sized + salsa::Database,
            String: zalsa_::interned::HashEqLike<T0>,
        {
            let current_revision = zalsa_::current_revision(db);
            Configuration_::ingredient(db).intern(
                db.as_dyn_database(),
                StructKey::<'db>(data, std::marker::PhantomData::default()),
                |id, data| {
                    StructData(
                        zalsa_::interned::Lookup::into_owned(data.0),
                        other(zalsa_::FromId::from_id(id)),
                    )
                },
            )
        }
        fn data<Db_>(self, db: &'db Db_) -> String
        where
            Db_: ?Sized + zalsa_::Database,
        {
            let fields = Configuration_::ingredient(db).fields(db.as_dyn_database(), self);
            std::clone::Clone::clone((&fields.0))
        }
        fn other<Db_>(self, db: &'db Db_) -> InternedString<'db>
        where
            Db_: ?Sized + zalsa_::Database,
        {
            let fields = Configuration_::ingredient(db).fields(db.as_dyn_database(), self);
            std::clone::Clone::clone((&fields.1))
        }
        #[doc = r" Default debug formatting for this struct (may be useful if you define your own `Debug` impl)"]
        pub fn default_debug_fmt(this: Self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            zalsa_::with_attached_database(|db| {
                let fields = Configuration_::ingredient(db).fields(db.as_dyn_database(), this);
                let mut f = f.debug_struct("InternedString");
                let f = f.field("data", &fields.0);
                let f = f.field("other", &fields.1);
                f.finish()
            })
            .unwrap_or_else(|| {
                f.debug_tuple("InternedString")
                    .field(&zalsa_::AsId::as_id(&this))
                    .finish()
            })
        }
    }
};
