use proc_macro2::TokenStream;

use crate::db_lifetime;
use crate::hygiene::Hygiene;
use crate::options::Options;
use crate::salsa_struct::{SalsaStruct, SalsaStructAllowedOptions};

/// For an entity struct `Foo` with fields `f1: T1, ..., fN: TN`, we generate...
///
/// * the "id struct" `struct Foo(salsa::Id)`
/// * the entity ingredient, which maps the id fields to the `Id`
/// * for each value field, a function ingredient
pub(crate) fn tracked_struct(
    args: proc_macro::TokenStream,
    struct_item: syn::ItemStruct,
) -> syn::Result<TokenStream> {
    let hygiene = Hygiene::from2(&struct_item);
    let m = Macro {
        hygiene,
        args: syn::parse(args)?,
        struct_item,
    };
    m.try_macro()
}

type TrackedArgs = Options<TrackedStruct>;

struct TrackedStruct;

impl crate::options::AllowedOptions for TrackedStruct {
    const RETURN_REF: bool = false;

    const SPECIFY: bool = false;

    const NO_EQ: bool = false;

    const DEBUG: bool = true;

    const NO_LIFETIME: bool = false;

    const NO_CLONE: bool = false;

    const SINGLETON: bool = true;

    const DATA: bool = true;

    const DB: bool = false;

    const CYCLE_FN: bool = false;

    const CYCLE_INITIAL: bool = false;

    const CYCLE_RESULT: bool = false;

    const LRU: bool = false;

    const CONSTRUCTOR_NAME: bool = true;

    const ID: bool = false;
}

impl SalsaStructAllowedOptions for TrackedStruct {
    const KIND: &'static str = "tracked";

    const ALLOW_TRACKED: bool = true;

    const HAS_LIFETIME: bool = true;

    const ELIDABLE_LIFETIME: bool = false;

    const ALLOW_DEFAULT: bool = false;
}

struct Macro {
    hygiene: Hygiene,
    args: TrackedArgs,
    struct_item: syn::ItemStruct,
}

impl Macro {
    #[allow(non_snake_case)]
    fn try_macro(&self) -> syn::Result<TokenStream> {
        let salsa_struct = SalsaStruct::new(&self.struct_item, &self.args)?;

        let attrs = &self.struct_item.attrs;
        let vis = &self.struct_item.vis;
        let struct_ident = &self.struct_item.ident;
        let db_lt = db_lifetime::db_lifetime(&self.struct_item.generics);
        let new_fn = salsa_struct.constructor_name();

        let field_ids = salsa_struct.field_ids();
        let tracked_ids = salsa_struct.tracked_ids();

        let tracked_vis = salsa_struct.tracked_vis();
        let untracked_vis = salsa_struct.untracked_vis();

        let tracked_getter_ids = salsa_struct.tracked_getter_ids();
        let untracked_getter_ids = salsa_struct.untracked_getter_ids();

        let field_indices = salsa_struct.field_indices();

        let absolute_tracked_indices = salsa_struct.tracked_field_indices();
        let relative_tracked_indices = (0..absolute_tracked_indices.len()).collect::<Vec<_>>();

        let absolute_untracked_indices = salsa_struct.untracked_field_indices();

        let tracked_options = salsa_struct.tracked_options();
        let untracked_options = salsa_struct.untracked_options();

        let field_tys = salsa_struct.field_tys();
        let tracked_tys = salsa_struct.tracked_tys();
        let untracked_tys = salsa_struct.untracked_tys();

        let num_tracked_fields = salsa_struct.num_tracked_fields();
        let generate_debug_impl = salsa_struct.generate_debug_impl();

        let zalsa = self.hygiene.ident("zalsa");
        let zalsa_struct = self.hygiene.ident("zalsa_struct");
        let Configuration = self.hygiene.ident("Configuration");
        let CACHE = self.hygiene.ident("CACHE");
        let Db = self.hygiene.ident("Db");
        let NonNull = self.hygiene.ident("NonNull");
        let Revision = self.hygiene.ident("Revision");

        Ok(crate::debug::dump_tokens(
            struct_ident,
            quote! {
                salsa::plumbing::setup_tracked_struct!(
                    attrs: [#(#attrs),*],
                    vis: #vis,
                    Struct: #struct_ident,
                    db_lt: #db_lt,
                    new_fn: #new_fn,

                    field_ids: [#(#field_ids),*],
                    tracked_ids: [#(#tracked_ids),*],

                    tracked_getters: [#(#tracked_vis #tracked_getter_ids),*],
                    untracked_getters: [#(#untracked_vis #untracked_getter_ids),*],

                    field_tys: [#(#field_tys),*],
                    tracked_tys: [#(#tracked_tys),*],
                    untracked_tys: [#(#untracked_tys),*],

                    field_indices: [#(#field_indices),*],

                    absolute_tracked_indices: [#(#absolute_tracked_indices),*],
                    relative_tracked_indices: [#(#relative_tracked_indices),*],

                    absolute_untracked_indices: [#(#absolute_untracked_indices),*],

                    tracked_options: [#(#tracked_options),*],
                    untracked_options: [#(#untracked_options),*],

                    num_tracked_fields: #num_tracked_fields,
                    generate_debug_impl: #generate_debug_impl,
                    unused_names: [
                        #zalsa,
                        #zalsa_struct,
                        #Configuration,
                        #CACHE,
                        #Db,
                        #NonNull,
                        #Revision,
                    ]
                );
            },
        ))
    }
}
