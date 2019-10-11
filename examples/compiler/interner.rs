use crate::values::*;

#[salsa::query_group(InternerDatabase)]
pub trait Interner: salsa::Database {
    #[salsa::interned]
    fn intern_field(&self, field: FieldData) -> Field;

    #[salsa::interned]
    fn intern_class(&self, class: ClassData) -> Class;
}
