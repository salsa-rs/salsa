#[derive(Copy, Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct Things<'db> {
    v: std::marker::PhantomData<&'db ()>,
}

#[salsa::interned]
struct Other {
    b: bool,
}

#[salsa::tracked]
impl Things<'_> {
    #[salsa::tracked(returns(ref))]
    pub fn implicit(db: &dyn salsa::Database, _: Other<'_>) -> Things<'_> {
        _ = db;
        todo!()
    }

    #[salsa::tracked]
    pub fn implicit2(db: &dyn salsa::Database, _: Other<'_>) -> Things<'_> {
        _ = db;
        todo!()
    }
}

#[salsa::tracked]
impl<'db> Things<'db> {
    #[salsa::tracked(returns(ref))]
    pub fn explicit(db: &'db dyn salsa::Database, _: Other<'db>) -> Things<'db> {
        _ = db;
        todo!()
    }

    #[salsa::tracked]
    pub fn explicit2(db: &'db dyn salsa::Database, _: Other<'db>) -> Things<'db> {
        _ = db;
        todo!()
    }

    #[salsa::tracked(returns(ref))]
    pub fn implicit3(db: &dyn salsa::Database, _: Other<'_>) -> Things<'_> {
        _ = db;
        todo!()
    }

    #[salsa::tracked]
    pub fn implicit4(db: &dyn salsa::Database, _: Other<'_>) -> Things<'_> {
        _ = db;
        todo!()
    }

    #[salsa::tracked(returns(copy))]
    pub fn static_value(db: &dyn salsa::Database) -> &'static str {
        _ = db;
        "test"
    }

    #[salsa::tracked]
    pub fn static_value2(db: &dyn salsa::Database) -> &'static str {
        _ = db;
        "test"
    }
}

fn main() {}
