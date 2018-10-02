mod class_table;
mod compiler;
mod implementation;

use self::class_table::ClassTableQueryContext;
use self::implementation::QueryContextImpl;

#[test]
fn test() {
    let query = QueryContextImpl::default();
    let all_def_ids = query.all_fields().read();
    assert_eq!(
        format!("{:?}", all_def_ids),
        "[DefId(1), DefId(2), DefId(11), DefId(12)]"
    );
}

fn main() {
    let query = QueryContextImpl::default();
    for f in query.all_fields().read().iter() {
        println!("{:?}", f);
    }
}
