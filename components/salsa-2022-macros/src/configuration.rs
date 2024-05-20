pub(crate) struct Configuration {
    pub(crate) db_lt: syn::Lifetime,
    pub(crate) jar_ty: syn::Type,
    pub(crate) salsa_struct_ty: syn::Type,
    pub(crate) input_ty: syn::Type,
    pub(crate) value_ty: syn::Type,
    pub(crate) cycle_strategy: CycleRecoveryStrategy,
    pub(crate) backdate_fn: syn::ImplItemFn,
    pub(crate) execute_fn: syn::ImplItemFn,
    pub(crate) recover_fn: syn::ImplItemFn,
}

impl Configuration {
    pub(crate) fn to_impl(&self, self_ty: &syn::Type) -> syn::ItemImpl {
        let Configuration {
            db_lt,
            jar_ty,
            salsa_struct_ty,
            input_ty,
            value_ty,
            cycle_strategy,
            backdate_fn,
            execute_fn,
            recover_fn,
        } = self;
        parse_quote! {
            impl salsa::function::Configuration for #self_ty {
                type Jar = #jar_ty;
                type SalsaStruct<#db_lt> = #salsa_struct_ty;
                type Input<#db_lt> = #input_ty;
                type Value<#db_lt> = #value_ty;
                const CYCLE_STRATEGY: salsa::cycle::CycleRecoveryStrategy = #cycle_strategy;
                #backdate_fn
                #execute_fn
                #recover_fn
            }
        }
    }
}

pub(crate) enum CycleRecoveryStrategy {
    Panic,
    Fallback,
}

impl quote::ToTokens for CycleRecoveryStrategy {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            CycleRecoveryStrategy::Panic => {
                tokens.extend(quote! {salsa::cycle::CycleRecoveryStrategy::Panic})
            }
            CycleRecoveryStrategy::Fallback => {
                tokens.extend(quote! {salsa::cycle::CycleRecoveryStrategy::Fallback})
            }
        }
    }
}

/// Returns an appropriate definition for `should_backdate_value` depending on
/// whether this value is memoized or not.
pub(crate) fn should_backdate_value_fn(should_backdate: bool) -> syn::ImplItemFn {
    if should_backdate {
        parse_quote! {
            fn should_backdate_value(v1: &Self::Value<'_>, v2: &Self::Value<'_>) -> bool {
                salsa::function::should_backdate_value(v1, v2)
            }
        }
    } else {
        parse_quote! {
            fn should_backdate_value(_v1: &Self::Value<'_>, _v2: &Self::Value<'_>) -> bool {
                false
            }
        }
    }
}

/// Returns an appropriate definition for `recover_from_cycle` for cases where
/// the cycle recovery is panic.
pub(crate) fn panic_cycle_recovery_fn() -> syn::ImplItemFn {
    parse_quote! {
        fn recover_from_cycle<'db>(
            _db: &'db salsa::function::DynDb<'db, Self>,
            _cycle: &salsa::Cycle,
            _key: salsa::Id,
        ) -> Self::Value<'db> {
            panic!()
        }
    }
}

pub(crate) fn value_ty(sig: &syn::Signature) -> syn::Type {
    match &sig.output {
        syn::ReturnType::Default => parse_quote!(()),
        syn::ReturnType::Type(_, ty) => syn::Type::clone(ty),
    }
}
