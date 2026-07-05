#[salsa::input]
struct FirstInput {
    #[returns(copy)]
    value: u32,
}

#[salsa::input]
struct SecondInput {
    #[returns(copy)]
    value: u32,
}

#[test]
#[should_panic(expected = "page belongs to ingredient")]
fn field_access_rejects_id_owned_by_another_ingredient() {
    let db = salsa::DatabaseImpl::default();
    let _first = FirstInput::new(&db, 1);
    let second = SecondInput::new(&db, 2);

    let forged =
        <FirstInput as salsa::plumbing::FromId>::from_id(salsa::plumbing::AsId::as_id(&second));

    forged.value(&db);
}
