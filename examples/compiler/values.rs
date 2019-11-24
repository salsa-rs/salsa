#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ClassData {
    pub fields: Vec<Field>,
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct Class(salsa::InternId);

impl salsa::InternKey for Class {
    fn from_intern_id(id: salsa::InternId) -> Self {
        Self(id)
    }

    fn as_intern_id(&self) -> salsa::InternId {
        self.0
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct FieldData {
    pub name: String,
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct Field(salsa::InternId);

impl salsa::InternKey for Field {
    fn from_intern_id(id: salsa::InternId) -> Self {
        Self(id)
    }

    fn as_intern_id(&self) -> salsa::InternId {
        self.0
    }
}
