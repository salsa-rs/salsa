//! Tests that we can create a database with very large jars without invoking UB

use salsa::storage::HasJars;

#[salsa::db(jar1::Jar1, jar2::Jar2, jar3::Jar3, jar4::Jar4)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

#[test]
fn execute() {
    let db = Database::default();
    let jars = db.storage.jars().0;

    ensure_init(jars);
}

fn ensure_init(place: *const <Database as HasJars>::Jars) {
    use std::mem::forget;
    use std::ptr::addr_of;

    // SAFETY: Intentionally tries to access potentially uninitialized memory,
    // so that miri can catch if we accidentally forget to initialize the memory.
    forget(unsafe { addr_of!((*place).0).read() });
    forget(unsafe { addr_of!((*place).1).read() });
    forget(unsafe { addr_of!((*place).2).read() });
    forget(unsafe { addr_of!((*place).3).read() });
}

macro_rules! make_jarX {
    ($jarX:ident, $JarX:ident) => {
        mod $jarX {
            #[salsa::jar(db = Db)]
            pub(crate) struct $JarX(T1);

            pub(crate) trait Db: salsa::DbWithJar<$JarX> {}

            impl<DB> Db for DB where DB: salsa::DbWithJar<$JarX> {}

            #[salsa::tracked(jar = $JarX)]
            struct T1 {
                a0: String,
                a1: String,
                a2: String,
                a3: String,
                a4: String,
                a5: String,
                a6: String,
                a7: String,
                a8: String,
                a9: String,
                a10: String,
                a11: String,
                a12: String,
                a13: String,
                a14: String,
                a15: String,
                a16: String,
                a17: String,
                a18: String,
                a19: String,
                a20: String,
                a21: String,
                a22: String,
                a23: String,
                a24: String,
                a25: String,
                a26: String,
                a27: String,
                a28: String,
                a29: String,
                a30: String,
                a31: String,
                a32: String,
                a33: String,
                a34: String,
                a35: String,
                a36: String,
                a37: String,
                a38: String,
                a39: String,
                a40: String,
                a41: String,
                a42: String,
                a43: String,
                a44: String,
                a45: String,
                a46: String,
                a47: String,
                a48: String,
                a49: String,
                a50: String,
                a51: String,
                a52: String,
                a53: String,
                a54: String,
                a55: String,
                a56: String,
                a57: String,
                a58: String,
                a59: String,
                a60: String,
                a61: String,
                a62: String,
                a63: String,
                a64: String,
                a65: String,
                a66: String,
                a67: String,
                a68: String,
                a69: String,
                a70: String,
                a71: String,
                a72: String,
                a73: String,
                a74: String,
                a75: String,
                a76: String,
                a77: String,
                a78: String,
                a79: String,
                a80: String,
                a81: String,
                a82: String,
                a83: String,
                a84: String,
                a85: String,
                a86: String,
                a87: String,
                a88: String,
                a89: String,
                a90: String,
                a91: String,
                a92: String,
                a93: String,
                a94: String,
                a95: String,
                a96: String,
                a97: String,
                a98: String,
                a99: String,
                a100: String,
            }
        }
    };
}

make_jarX!(jar1, Jar1);
make_jarX!(jar2, Jar2);
make_jarX!(jar3, Jar3);
make_jarX!(jar4, Jar4);
