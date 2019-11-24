use std::sync::Arc;

use crate::{interner::Interner, values::*};

#[salsa::query_group(CompilerDatabase)]
pub trait Compiler: Interner {
    #[salsa::input]
    fn input_string(&self) -> Arc<String>;

    /// Get the fields.
    fn fields(&self, class: Class) -> Arc<Vec<Field>>;

    /// Get the list of all classes
    fn all_classes(&self) -> Arc<Vec<Class>>;

    /// Get the list of all fields
    fn all_fields(&self) -> Arc<Vec<Field>>;
}

/// This function parses a dummy language with the following structure:
///
/// Classes are defined one per line, consisting of a comma-separated list of fields.
///
/// Example:
///
/// ```
/// lorem,ipsum
/// dolor,sit,amet,
/// consectetur,adipiscing,elit
/// ```
fn all_classes(db: &impl Compiler) -> Arc<Vec<Class>> {
    let string = db.input_string();

    let rows = string.split('\n');
    let classes: Vec<_> = rows
        .filter(|string| !string.is_empty())
        .map(|string| {
            let fields = string
                .trim()
                .split(',')
                .filter(|string| !string.is_empty())
                .map(|name_str| {
                    let name = name_str.to_owned();
                    let field_data = FieldData { name };
                    db.intern_field(field_data)
                })
                .collect();
            let class_data = ClassData { fields };
            db.intern_class(class_data)
        })
        .collect();

    Arc::new(classes)
}

fn fields(db: &impl Compiler, class: Class) -> Arc<Vec<Field>> {
    let class = db.lookup_intern_class(class);
    let fields = class.fields.clone();
    Arc::new(fields)
}

fn all_fields(db: &impl Compiler) -> Arc<Vec<Field>> {
    Arc::new(
        db.all_classes()
            .iter()
            .cloned()
            .flat_map(|class| {
                let fields = db.fields(class);
                (0..fields.len()).map(move |i| fields[i])
            })
            .collect(),
    )
}
