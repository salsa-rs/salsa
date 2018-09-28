mod class_table;
mod compiler;
mod implementation;

use self::class_table::ClassTableQueryContext;
use self::implementation::QueryContextImpl;

fn main() {
    let query = QueryContextImpl::default();
    for f in query.all_fields().of(()).iter() {
        println!("{:?}", f);
    }
}
