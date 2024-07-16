#[salsa::jar(db = Db)]
struct Jar(AccTwoUnnamedFields, AccNamedField);

trait Db: salsa::DbWithJar<Jar> {}

// accumulator with more than one unnamed fields
#[salsa::accumulator]
struct AccTwoUnnamedFields (u32, u32);


// accumulator with named fields
#[salsa::accumulator]
struct AccNamedField {
    field: u32,
}

fn main() {}