struct BadConfiguration;

impl salsa::plumbing::interned::Configuration for BadConfiguration {
    const LOCATION: salsa::plumbing::Location = salsa::plumbing::Location {
        file: file!(),
        line: line!(),
    };
    const DEBUG_NAME: &'static str = "BadConfiguration";
    const PERSIST: bool = false;

    type Fields<'db> = &'db u32;
    type Struct<'db> = salsa::Id;

    fn serialize<S>(
        _value: &Self::Fields<'_>,
        _serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: salsa::plumbing::serde::Serializer,
    {
        unimplemented!()
    }

    fn deserialize<'de, D>(_deserializer: D) -> Result<Self::Fields<'static>, D::Error>
    where
        D: salsa::plumbing::serde::Deserializer<'de>,
    {
        unimplemented!()
    }
}

fn main() {}
