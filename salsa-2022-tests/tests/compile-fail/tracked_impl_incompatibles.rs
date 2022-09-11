#[salsa::jar(db = Db)]
struct Jar(MyTracked);


#[salsa::tracked]
struct MyTracked {
    field: u32,
}

#[salsa::tracked(return_ref)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}
#[salsa::tracked(specify)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}

#[salsa::tracked(no_eq)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}

#[salsa::tracked(data = Data)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}

#[salsa::tracked(db = Db)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}

#[salsa::tracked(recover_fn = recover)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}

#[salsa::tracked(lru = 32)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}

#[salsa::tracked(constructor = Constructor)]
impl std::default::Default for MyTracked {
    fn default() -> Self {

    }
}
#[salsa::tracked]
impl std::default::Default for [MyTracked; 12] {
    fn default() -> Self {

    }
}


trait Db: salsa::DbWithJar<Jar> {}

fn main() {}