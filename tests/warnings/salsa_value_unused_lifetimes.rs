use std::marker::PhantomData;

pub struct NotSalsaValue<'db>(pub PhantomData<&'db ()>);

#[derive(salsa::SalsaValue)]
pub struct ManualRetentionProof<'db> {
    #[salsa_value(unsafe(prove_safe_to_retain_manually))]
    pub value: NotSalsaValue<'db>,
}

#[test]
fn manual_retention_proof_does_not_warn_about_unused_lifetimes() {
    let value = ManualRetentionProof {
        value: NotSalsaValue(PhantomData),
    };
    let _ = value.value.0;
}
