mod class_table;
mod compiler;
mod implementation;

use self::class_table::ClassTableDatabase;
use self::implementation::DatabaseImpl;

#[test]
fn test() {
    let query = DatabaseImpl::default();
    let all_def_ids = query.all_fields(());
    assert_eq!(
        format!("{:?}", all_def_ids),
        "[DefId(1), DefId(2), DefId(11), DefId(12)]"
    );
}

fn main() {
    let query = DatabaseImpl::default();
    for f in query.all_fields(()).iter() {
        println!("{:?}", f);
    }
}
