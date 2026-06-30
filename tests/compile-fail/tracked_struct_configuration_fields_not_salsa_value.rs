struct BadConfiguration;

impl salsa::plumbing::tracked_struct::Configuration for BadConfiguration {
    const LOCATION: salsa::plumbing::Location = salsa::plumbing::Location {
        file: file!(),
        line: line!(),
    };
    const DEBUG_NAME: &'static str = "BadConfiguration";
    const TRACKED_FIELD_NAMES: &'static [&'static str] = &[];
    const TRACKED_FIELD_INDICES: &'static [usize] = &[];
    const PERSIST: bool = false;

    type Fields<'db> = &'db u32;
    type Revisions = [salsa::plumbing::AtomicRevision; 0];
    type Struct<'db> = salsa::Id;

    fn untracked_fields(_fields: &Self::Fields<'_>) -> impl std::hash::Hash {}

    fn new_revisions(_current_revision: salsa::Revision) -> Self::Revisions {
        []
    }

    fn update_fields<'db>(
        _current_revision: salsa::Revision,
        _revisions: &Self::Revisions,
        old_fields: &mut Self::Fields<'db>,
        new_fields: Self::Fields<'db>,
    ) -> bool {
        *old_fields = new_fields;
        true
    }

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
