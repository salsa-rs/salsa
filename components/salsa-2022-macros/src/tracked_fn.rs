use proc_macro2::{Literal, TokenStream};
use syn::spanned::Spanned;
use syn::visit_mut::VisitMut;
use syn::{ReturnType, Token};

use crate::configuration::{self, Configuration, CycleRecoveryStrategy};
use crate::options::Options;

pub(crate) fn tracked_fn(
    args: proc_macro::TokenStream,
    mut item_fn: syn::ItemFn,
) -> syn::Result<TokenStream> {
    let args: FnArgs = syn::parse(args)?;
    if item_fn.sig.inputs.is_empty() {
        return Err(syn::Error::new(
            item_fn.sig.ident.span(),
            "tracked functions must have at least a database argument",
        ));
    }

    if let syn::FnArg::Receiver(receiver) = &item_fn.sig.inputs[0] {
        return Err(syn::Error::new(
            receiver.span(),
            "#[salsa::tracked] must also be applied to the impl block for tracked methods",
        ));
    }

    if let Some(s) = &args.specify {
        if function_type(&item_fn) == FunctionType::RequiresInterning {
            return Err(syn::Error::new(
                s.span(),
                "tracked function takes too many arguments to have its value set with `specify`",
            ));
        }

        if args.lru.is_some() {
            return Err(syn::Error::new(
                s.span(),
                "`specify` and `lru` cannot be used together",
            ));
        }
    }

    let (config_ty, fn_struct) = fn_struct(&args, &item_fn)?;
    *item_fn.block = getter_fn(&args, &mut item_fn.sig, item_fn.block.span(), &config_ty)?;

    Ok(quote! {
        #fn_struct

        // we generate a `'db` lifetime that clippy
        // sometimes doesn't like
        #[allow(clippy::needless_lifetimes)]
        #item_fn
    })
}

type FnArgs = Options<TrackedFn>;

struct TrackedFn;

impl crate::options::AllowedOptions for TrackedFn {
    const RETURN_REF: bool = true;

    const SPECIFY: bool = true;

    const NO_EQ: bool = true;

    const SINGLETON: bool = false;

    const JAR: bool = true;

    const DATA: bool = false;

    const DB: bool = false;

    const RECOVERY_FN: bool = true;

    const LRU: bool = true;

    const CONSTRUCTOR_NAME: bool = false;
}

type ImplArgs = Options<TrackedImpl>;

pub(crate) fn tracked_impl(
    args: proc_macro::TokenStream,
    mut item_impl: syn::ItemImpl,
) -> syn::Result<TokenStream> {
    let args: ImplArgs = syn::parse(args)?;
    let self_type = match &*item_impl.self_ty {
        syn::Type::Path(path) => path,
        _ => {
            return Err(syn::Error::new(
                item_impl.self_ty.span(),
                "#[salsa::tracked] can only be applied to salsa structs",
            ))
        }
    };
    let self_type_name = &self_type.path.segments.last().unwrap().ident;
    let name_prefix = match &item_impl.trait_ {
        Some((_, trait_name, _)) => format!(
            "{}_{}",
            self_type_name,
            trait_name.segments.last().unwrap().ident
        ),
        None => format!("{}", self_type_name),
    };
    let extra_impls = item_impl
        .items
        .iter_mut()
        .filter_map(|item| {
            let item_method = match item {
                syn::ImplItem::Method(item_method) => item_method,
                _ => return None,
            };
            let salsa_tracked_attr = item_method.attrs.iter().position(|attr| {
                let path = &attr.path.segments;
                path.len() == 2
                    && path[0].arguments == syn::PathArguments::None
                    && path[0].ident == "salsa"
                    && path[1].arguments == syn::PathArguments::None
                    && path[1].ident == "tracked"
            })?;
            let salsa_tracked_attr = item_method.attrs.remove(salsa_tracked_attr);
            let inner_args = if !salsa_tracked_attr.tokens.is_empty() {
                salsa_tracked_attr.parse_args()
            } else {
                Ok(FnArgs::default())
            };
            let inner_args = match inner_args {
                Ok(inner_args) => inner_args,
                Err(err) => return Some(Err(err)),
            };
            let name = format!("{}_{}", name_prefix, item_method.sig.ident);
            Some(tracked_method(
                &args,
                inner_args,
                item_method,
                self_type,
                &name,
            ))
        })
        // Collate all the errors so we can display them all at once
        .fold(Ok(Vec::new()), |mut acc, res| {
            match (&mut acc, res) {
                (Ok(extra_impls), Ok(impls)) => extra_impls.push(impls),
                (Ok(_), Err(err)) => acc = Err(err),
                (Err(_), Ok(_)) => {}
                (Err(errors), Err(err)) => errors.combine(err),
            }
            acc
        })?;

    Ok(quote! {
        #item_impl

        #(#extra_impls)*
    })
}

struct TrackedImpl;

impl crate::options::AllowedOptions for TrackedImpl {
    const RETURN_REF: bool = false;

    const SPECIFY: bool = false;

    const NO_EQ: bool = false;

    const JAR: bool = true;

    const DATA: bool = false;

    const DB: bool = false;

    const RECOVERY_FN: bool = false;

    const LRU: bool = false;

    const CONSTRUCTOR_NAME: bool = false;

    const SINGLETON: bool = false;
}

fn tracked_method(
    outer_args: &ImplArgs,
    mut args: FnArgs,
    item_method: &mut syn::ImplItemMethod,
    self_type: &syn::TypePath,
    name: &str,
) -> syn::Result<TokenStream> {
    args.jar_ty = args.jar_ty.or_else(|| outer_args.jar_ty.clone());

    if item_method.sig.inputs.len() <= 1 {
        return Err(syn::Error::new(
            item_method.sig.ident.span(),
            "tracked methods must have at least self and a database argument",
        ));
    }

    let mut item_fn = syn::ItemFn {
        attrs: item_method.attrs.clone(),
        vis: item_method.vis.clone(),
        sig: item_method.sig.clone(),
        block: Box::new(rename_self_in_block(item_method.block.clone())?),
    };
    item_fn.sig.ident = syn::Ident::new(name, item_fn.sig.ident.span());
    // Flip the first and second arguments as the rest of the code expects the
    // database to come first and the struct to come second. We also need to
    // change the self argument to a normal typed argument called __salsa_self.
    let mut original_inputs = item_fn.sig.inputs.into_pairs();
    let self_param = match original_inputs.next().unwrap().into_value() {
        syn::FnArg::Receiver(r) if r.reference.is_none() => r,
        arg => return Err(syn::Error::new(arg.span(), "first argument must be self")),
    };
    let db_param = original_inputs.next().unwrap().into_value();
    let mut inputs = syn::punctuated::Punctuated::new();
    inputs.push(db_param);
    inputs.push(syn::FnArg::Typed(syn::PatType {
        attrs: self_param.attrs,
        pat: Box::new(syn::Pat::Ident(syn::PatIdent {
            attrs: Vec::new(),
            by_ref: None,
            mutability: self_param.mutability,
            ident: syn::Ident::new("__salsa_self", self_param.self_token.span),
            subpat: None,
        })),
        colon_token: Default::default(),
        ty: Box::new(syn::Type::Path(self_type.clone())),
    }));
    inputs.push_punct(Default::default());
    inputs.extend(original_inputs);
    item_fn.sig.inputs = inputs;

    let (config_ty, fn_struct) = crate::tracked_fn::fn_struct(&args, &item_fn)?;

    item_method.block = getter_fn(
        &args,
        &mut item_method.sig,
        item_method.block.span(),
        &config_ty,
    )?;

    Ok(fn_struct)
}

/// Rename all occurrences of `self` to `__salsa_self` in a block
/// so that it can be used in a free function.
fn rename_self_in_block(mut block: syn::Block) -> syn::Result<syn::Block> {
    struct RenameIdent(syn::Result<()>);

    impl syn::visit_mut::VisitMut for RenameIdent {
        fn visit_ident_mut(&mut self, i: &mut syn::Ident) {
            if i == "__salsa_self" {
                let err = syn::Error::new(
                    i.span(),
                    "Existing variable name clashes with 'self' -> '__salsa_self' renaming",
                );
                match &mut self.0 {
                    Ok(()) => self.0 = Err(err),
                    Err(errors) => errors.combine(err),
                }
            }
            if i == "self" {
                *i = syn::Ident::new("__salsa_self", i.span());
            }
        }
    }

    let mut rename = RenameIdent(Ok(()));
    rename.visit_block_mut(&mut block);
    rename.0.map(move |()| block)
}

/// Create the struct representing the function and all of its impls.
///
/// This returns the name of the constructed type and the code defining everything.
fn fn_struct(args: &FnArgs, item_fn: &syn::ItemFn) -> syn::Result<(syn::Type, TokenStream)> {
    let struct_item = configuration_struct(item_fn);
    let configuration = fn_configuration(args, item_fn);
    let struct_item_ident = &struct_item.ident;
    let config_ty: syn::Type = parse_quote!(#struct_item_ident);
    let configuration_impl = configuration.to_impl(&config_ty);
    let ingredients_for_impl = ingredients_for_impl(args, item_fn, &config_ty);
    let item_impl = setter_impl(args, item_fn, &config_ty)?;

    Ok((
        config_ty,
        quote! {
            #struct_item
            #configuration_impl
            #ingredients_for_impl
            #item_impl
        },
    ))
}

/// Returns the key type for this tracked function.
/// This is a tuple of all the argument types (apart from the database).
fn key_tuple_ty(item_fn: &syn::ItemFn) -> syn::Type {
    let arg_tys = item_fn.sig.inputs.iter().skip(1).map(|arg| match arg {
        syn::FnArg::Receiver(_) => unreachable!(),
        syn::FnArg::Typed(pat_ty) => pat_ty.ty.clone(),
    });

    parse_quote!(
        (#(#arg_tys,)*)
    )
}

fn configuration_struct(item_fn: &syn::ItemFn) -> syn::ItemStruct {
    let fn_name = item_fn.sig.ident.clone();
    let visibility = &item_fn.vis;

    let salsa_struct_ty = salsa_struct_ty(item_fn);
    let intern_map: syn::Type = match function_type(item_fn) {
        FunctionType::Constant => {
            parse_quote! { salsa::interned::IdentityInterner<()> }
        }
        FunctionType::SalsaStruct => {
            parse_quote! { salsa::interned::IdentityInterner<#salsa_struct_ty> }
        }
        FunctionType::RequiresInterning => {
            let key_ty = key_tuple_ty(item_fn);
            parse_quote! { salsa::interned::InternedIngredient<salsa::Id, #key_ty> }
        }
    };

    parse_quote! {
        #[allow(non_camel_case_types)]
        #visibility struct #fn_name {
            intern_map: #intern_map,
            function: salsa::function::FunctionIngredient<Self>,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
enum FunctionType {
    Constant,
    SalsaStruct,
    RequiresInterning,
}

fn function_type(item_fn: &syn::ItemFn) -> FunctionType {
    match item_fn.sig.inputs.len() {
        0 => unreachable!(
            "functions have been checked to have at least a database argument by this point"
        ),
        1 => FunctionType::Constant,
        2 => FunctionType::SalsaStruct,
        _ => FunctionType::RequiresInterning,
    }
}

/// Every tracked fn takes a salsa struct as its second argument.
/// This fn returns the type of that second argument.
fn salsa_struct_ty(item_fn: &syn::ItemFn) -> syn::Type {
    if item_fn.sig.inputs.len() == 1 {
        return parse_quote! { salsa::salsa_struct::Singleton };
    }
    match &item_fn.sig.inputs[1] {
        syn::FnArg::Receiver(_) => panic!("receiver not expected"),
        syn::FnArg::Typed(pat_ty) => (*pat_ty.ty).clone(),
    }
}

fn fn_configuration(args: &FnArgs, item_fn: &syn::ItemFn) -> Configuration {
    let jar_ty = args.jar_ty();
    let salsa_struct_ty = salsa_struct_ty(item_fn);
    let key_ty = match function_type(item_fn) {
        FunctionType::Constant => parse_quote!(()),
        FunctionType::SalsaStruct => salsa_struct_ty.clone(),
        FunctionType::RequiresInterning => parse_quote!(salsa::id::Id),
    };
    let value_ty = configuration::value_ty(&item_fn.sig);

    let fn_ty = item_fn.sig.ident.clone();

    let indices = (0..item_fn.sig.inputs.len() - 1).map(Literal::usize_unsuffixed);
    let (cycle_strategy, recover_fn) = if let Some(recovery_fn) = &args.recovery_fn {
        // Create the `recover_from_cycle` function, which (a) maps from the interned id to the actual
        // keys and then (b) invokes the recover function itself.
        let cycle_strategy = CycleRecoveryStrategy::Fallback;

        let cycle_fullback = parse_quote! {
            fn recover_from_cycle(__db: &salsa::function::DynDb<Self>, __cycle: &salsa::Cycle, __id: Self::Key) -> Self::Value {
                let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
                let __ingredients =
                    <_ as salsa::storage::HasIngredientsFor<#fn_ty>>::ingredient(__jar);
                let __key = __ingredients.intern_map.data(__runtime, __id).clone();
                #recovery_fn(__db, __cycle, #(__key.#indices),*)
            }
        };
        (cycle_strategy, cycle_fullback)
    } else {
        // When the `recovery_fn` attribute is not set, set `cycle_strategy` to `Panic`
        let cycle_strategy = CycleRecoveryStrategy::Panic;
        let cycle_panic = configuration::panic_cycle_recovery_fn();
        (cycle_strategy, cycle_panic)
    };

    let backdate_fn = configuration::should_backdate_value_fn(args.should_backdate());

    // The type of the configuration struct; this has the same name as the fn itself.

    // Make a copy of the fn with a different name; we will invoke this from `execute`.
    // We need to change the name because, otherwise, if the function invoked itself
    // recursively it would not go through the query system.
    let inner_fn_name = &syn::Ident::new("__fn", item_fn.sig.ident.span());
    let mut inner_fn = item_fn.clone();
    inner_fn.sig.ident = inner_fn_name.clone();

    // Create the `execute` function, which (a) maps from the interned id to the actual
    // keys and then (b) invokes the function itself (which we embed within).
    let indices = (0..item_fn.sig.inputs.len() - 1).map(Literal::usize_unsuffixed);
    let execute_fn = parse_quote! {
        fn execute(__db: &salsa::function::DynDb<Self>, __id: Self::Key) -> Self::Value {
            #inner_fn

            let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
            let __ingredients =
                <_ as salsa::storage::HasIngredientsFor<#fn_ty>>::ingredient(__jar);
            let __key = __ingredients.intern_map.data(__runtime, __id).clone();
            #inner_fn_name(__db, #(__key.#indices),*)
        }
    };

    Configuration {
        jar_ty,
        salsa_struct_ty,
        key_ty,
        value_ty,
        cycle_strategy,
        backdate_fn,
        execute_fn,
        recover_fn,
    }
}

fn ingredients_for_impl(
    args: &FnArgs,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::ItemImpl {
    let jar_ty = args.jar_ty();
    let debug_name = crate::literal(&item_fn.sig.ident);

    let intern_map: syn::Expr = match function_type(item_fn) {
        FunctionType::Constant | FunctionType::SalsaStruct => {
            parse_quote! {
                salsa::interned::IdentityInterner::new()
            }
        }
        FunctionType::RequiresInterning => {
            parse_quote! {
                {
                    let index = routes.push(
                        |jars| {
                            let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                            let ingredients =
                                <_ as salsa::storage::HasIngredientsFor<Self::Ingredients>>::ingredient(jar);
                            &ingredients.intern_map
                        },
                        |jars| {
                            let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                            let ingredients =
                                <_ as salsa::storage::HasIngredientsFor<Self::Ingredients>>::ingredient_mut(jar);
                            &mut ingredients.intern_map
                        }
                    );
                    salsa::interned::InternedIngredient::new(index, #debug_name)
                }
            }
        }
    };

    // set 0 as default to disable LRU
    let lru = args.lru.unwrap_or(0);

    // get the name of the function as a string literal
    let debug_name = crate::literal(&item_fn.sig.ident);

    parse_quote! {
        impl salsa::storage::IngredientsFor for #config_ty {
            type Ingredients = Self;
            type Jar = #jar_ty;

            fn create_ingredients<DB>(routes: &mut salsa::routes::Routes<DB>) -> Self::Ingredients
            where
                DB: salsa::DbWithJar<Self::Jar> + salsa::storage::JarFromJars<Self::Jar>,
            {
                Self {
                    intern_map: #intern_map,

                    function: {
                        let index = routes.push(
                            |jars| {
                                let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars(jars);
                                let ingredients =
                                    <_ as salsa::storage::HasIngredientsFor<Self::Ingredients>>::ingredient(jar);
                                &ingredients.function
                            },
                            |jars| {
                                let jar = <DB as salsa::storage::JarFromJars<Self::Jar>>::jar_from_jars_mut(jars);
                                let ingredients =
                                    <_ as salsa::storage::HasIngredientsFor<Self::Ingredients>>::ingredient_mut(jar);
                                &mut ingredients.function
                            });
                        let ingredient = salsa::function::FunctionIngredient::new(index, #debug_name);
                        ingredient.set_capacity(#lru);
                        ingredient
                    }
                }
            }
        }
    }
}

fn setter_impl(
    args: &FnArgs,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<syn::ItemImpl> {
    let ref_getter_fn = ref_getter_fn(args, item_fn, config_ty)?;
    let accumulated_fn = accumulated_fn(args, item_fn, config_ty)?;
    let setter_fn = setter_fn(args, item_fn, config_ty)?;
    let specify_fn = specify_fn(args, item_fn, config_ty)?.map(|f| quote! { #f });
    let set_lru_fn = set_lru_capacity_fn(args, config_ty)?.map(|f| quote! { #f });

    let setter_impl: syn::ItemImpl = parse_quote! {
        impl #config_ty {
            #[allow(dead_code, clippy::needless_lifetimes)]
            #ref_getter_fn

            #[allow(dead_code, clippy::needless_lifetimes)]
            #setter_fn

            #[allow(dead_code, clippy::needless_lifetimes)]
            #accumulated_fn

            #set_lru_fn

            #specify_fn
        }
    };

    Ok(setter_impl)
}

/// Creates the shim function that looks like the original function but calls
/// into the machinery we've just generated rather than executing the code.
fn getter_fn(
    args: &FnArgs,
    fn_sig: &mut syn::Signature,
    block_span: proc_macro2::Span,
    config_ty: &syn::Type,
) -> syn::Result<syn::Block> {
    let mut is_method = false;
    let mut arg_idents: Vec<_> = fn_sig
        .inputs
        .iter()
        .map(|arg| -> syn::Result<syn::Ident> {
            match arg {
                syn::FnArg::Receiver(receiver) => {
                    is_method = true;
                    Ok(syn::Ident::new("self", receiver.self_token.span()))
                }
                syn::FnArg::Typed(pat_ty) => Ok(match &*pat_ty.pat {
                    syn::Pat::Ident(ident) => ident.ident.clone(),
                    _ => return Err(syn::Error::new(arg.span(), "unsupported argument kind")),
                }),
            }
        })
        .collect::<Result<_, _>>()?;
    // If this is a method then the order of the database and the salsa struct are reversed
    // because the self argument must always come first.
    if is_method {
        arg_idents.swap(0, 1);
    }
    Ok(if args.return_ref.is_some() {
        make_fn_return_ref(fn_sig)?;
        parse_quote_spanned! {
            block_span => {
                #config_ty::get(#(#arg_idents,)*)
            }
        }
    } else {
        parse_quote_spanned! {
            block_span => {
                Clone::clone(#config_ty::get(#(#arg_idents,)*))
            }
        }
    })
}

/// Creates a `get` associated function that returns `&Value`
/// (to be used when `return_ref` is specified).
///
/// (Helper for `getter_fn`)
fn ref_getter_fn(
    args: &FnArgs,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<syn::ItemFn> {
    let jar_ty = args.jar_ty();
    let mut ref_getter_fn = item_fn.clone();
    ref_getter_fn.sig.ident = syn::Ident::new("get", item_fn.sig.ident.span());
    make_fn_return_ref(&mut ref_getter_fn.sig)?;

    let (db_var, arg_names) = fn_args(item_fn)?;
    ref_getter_fn.block = parse_quote! {
        {
            let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(#db_var);
            let __ingredients = <_ as salsa::storage::HasIngredientsFor<#config_ty>>::ingredient(__jar);
            let __key = __ingredients.intern_map.intern(__runtime, (#(#arg_names),*));
            __ingredients.function.fetch(#db_var, __key)
        }
    };

    Ok(ref_getter_fn)
}

/// Creates a `set` associated function that can be used to set (given an `&mut db`)
/// the value for this function for some inputs.
fn setter_fn(
    args: &FnArgs,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<syn::ImplItemMethod> {
    // The setter has *always* the same signature as the original:
    // but it takes a value arg and has no return type.
    let jar_ty = args.jar_ty();
    let (db_var, arg_names) = fn_args(item_fn)?;
    let mut setter_sig = item_fn.sig.clone();
    let value_ty = configuration::value_ty(&item_fn.sig);
    setter_sig.ident = syn::Ident::new("set", item_fn.sig.ident.span());
    match &mut setter_sig.inputs[0] {
        // change from `&dyn ...` to `&mut dyn...`
        syn::FnArg::Receiver(_) => unreachable!(), // early fns should have detected
        syn::FnArg::Typed(pat_ty) => match &mut *pat_ty.ty {
            syn::Type::Reference(ty) => {
                ty.mutability = Some(Token![mut](ty.and_token.span()));
            }
            _ => unreachable!(), // early fns should have detected
        },
    }
    let value_arg = syn::Ident::new("__value", item_fn.sig.output.span());
    setter_sig.inputs.push(parse_quote!(#value_arg: #value_ty));
    setter_sig.output = ReturnType::Default;
    Ok(syn::ImplItemMethod {
        attrs: vec![],
        vis: item_fn.vis.clone(),
        defaultness: None,
        sig: setter_sig,
        block: parse_quote! {
            {
                let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar_mut(#db_var);
                let __ingredients = <_ as salsa::storage::HasIngredientsFor<#config_ty>>::ingredient_mut(__jar);
                let __key = __ingredients.intern_map.intern(__runtime, (#(#arg_names),*));
                __ingredients.function.store(__runtime, __key, #value_arg, salsa::Durability::LOW)
            }
        },
    })
}

/// Create a `set_lru_capacity` associated function that can be used to change LRU
/// capacity at runtime.
/// Note that this function is only generated if the tracked function has the lru option set.
///
/// # Examples
///
/// ```rust,ignore
/// #[salsa::tracked(lru=32)]
/// fn my_tracked_fn(db: &dyn crate::Db, ...) { }
///
/// my_tracked_fn::set_lru_capacity(16)
/// ```
fn set_lru_capacity_fn(
    args: &FnArgs,
    config_ty: &syn::Type,
) -> syn::Result<Option<syn::ImplItemMethod>> {
    if args.lru.is_none() {
        return Ok(None);
    }

    let jar_ty = args.jar_ty();
    let lru_fn = parse_quote! {
        #[allow(dead_code, clippy::needless_lifetimes)]
        fn set_lru_capacity(__db: &salsa::function::DynDb<Self>, __value: usize) {
            let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(__db);
            let __ingredients =
                <_ as salsa::storage::HasIngredientsFor<#config_ty>>::ingredient(__jar);
            __ingredients.function.set_capacity(__value);
        }
    };
    Ok(Some(lru_fn))
}

fn specify_fn(
    args: &FnArgs,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<Option<syn::ImplItemMethod>> {
    if args.specify.is_none() {
        return Ok(None);
    }

    // `specify` has the same signature as the original,
    // but it takes a value arg and has no return type.
    let jar_ty = args.jar_ty();
    let (db_var, arg_names) = fn_args(item_fn)?;
    let mut setter_sig = item_fn.sig.clone();
    let value_ty = configuration::value_ty(&item_fn.sig);
    setter_sig.ident = syn::Ident::new("specify", item_fn.sig.ident.span());
    let value_arg = syn::Ident::new("__value", item_fn.sig.output.span());
    setter_sig.inputs.push(parse_quote!(#value_arg: #value_ty));
    setter_sig.output = ReturnType::Default;
    Ok(Some(syn::ImplItemMethod {
        attrs: vec![],
        vis: item_fn.vis.clone(),
        defaultness: None,
        sig: setter_sig,
        block: parse_quote! {
            {

                let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(#db_var);
                let __ingredients = <_ as salsa::storage::HasIngredientsFor<#config_ty>>::ingredient(__jar);
                __ingredients.function.specify_and_record(#db_var, #(#arg_names,)* #value_arg)
            }
        },
    }))
}
/// Given a function def tagged with `#[return_ref]`, modifies `fn_sig` so that
/// it returns an `&Value` instead of `Value`. May introduce a name for the
/// database lifetime if required.
fn make_fn_return_ref(mut fn_sig: &mut syn::Signature) -> syn::Result<()> {
    // An input should be a `&dyn Db`.
    // We need to ensure it has a named lifetime parameter.
    let (db_lifetime, _) = db_lifetime_and_ty(fn_sig)?;

    let (right_arrow, elem) = match fn_sig.output.clone() {
        ReturnType::Default => (syn::Token![->](fn_sig.paren_token.span), parse_quote!(())),
        ReturnType::Type(rarrow, ty) => (rarrow, ty),
    };

    let ref_output = syn::TypeReference {
        and_token: syn::Token![&](right_arrow.span()),
        lifetime: Some(db_lifetime),
        mutability: None,
        elem,
    };

    fn_sig.output = syn::ReturnType::Type(right_arrow, Box::new(ref_output.into()));

    Ok(())
}

/// Given a function signature, identifies the name given to the `&dyn Db` reference
/// and returns it, along with the type of the database.
/// If the database lifetime did not have a name, then modifies the item function
/// so that it is called `'__db` and returns that.
fn db_lifetime_and_ty(func: &mut syn::Signature) -> syn::Result<(syn::Lifetime, &syn::Type)> {
    // If this is a method, then the database should be the second argument.
    let db_loc = if matches!(func.inputs[0], syn::FnArg::Receiver(_)) {
        1
    } else {
        0
    };
    match &mut func.inputs[db_loc] {
        syn::FnArg::Receiver(r) => Err(syn::Error::new(r.span(), "two self arguments")),
        syn::FnArg::Typed(pat_ty) => match &mut *pat_ty.ty {
            syn::Type::Reference(ty) => match &ty.lifetime {
                Some(lt) => Ok((lt.clone(), &pat_ty.ty)),
                None => {
                    let and_token_span = ty.and_token.span();
                    let ident = syn::Ident::new("__db", and_token_span);
                    func.generics.params.insert(
                        0,
                        syn::LifetimeDef {
                            attrs: vec![],
                            lifetime: syn::Lifetime {
                                apostrophe: and_token_span,
                                ident: ident.clone(),
                            },
                            colon_token: None,
                            bounds: Default::default(),
                        }
                        .into(),
                    );
                    let db_lifetime = syn::Lifetime {
                        apostrophe: and_token_span,
                        ident,
                    };
                    ty.lifetime = Some(db_lifetime.clone());
                    Ok((db_lifetime, &pat_ty.ty))
                }
            },
            _ => Err(syn::Error::new(
                pat_ty.span(),
                "expected database to be a `&` type",
            )),
        },
    }
}

/// Generates the `accumulated` function, which invokes `accumulated`
/// on the function ingredient to extract the values pushed (transitively)
/// into an accumulator.
fn accumulated_fn(
    args: &FnArgs,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<syn::ItemFn> {
    let jar_ty = args.jar_ty();

    let mut accumulated_fn = item_fn.clone();
    accumulated_fn.sig.ident = syn::Ident::new("accumulated", item_fn.sig.ident.span());
    accumulated_fn.sig.generics.params.push(parse_quote! {
        __A: salsa::accumulator::Accumulator
    });
    accumulated_fn.sig.output = parse_quote! {
        -> Vec<<__A as salsa::accumulator::Accumulator>::Data>
    };

    let (db_lifetime, _) = db_lifetime_and_ty(&mut accumulated_fn.sig)?;
    let predicate: syn::WherePredicate = parse_quote!(<#jar_ty as salsa::jar::Jar<#db_lifetime>>::DynDb: salsa::storage::HasJar<<__A as salsa::accumulator::Accumulator>::Jar>);

    if let Some(where_clause) = &mut accumulated_fn.sig.generics.where_clause {
        where_clause.predicates.push(predicate);
    } else {
        accumulated_fn.sig.generics.where_clause = parse_quote!(where #predicate);
    }

    let (db_var, arg_names) = fn_args(item_fn)?;
    accumulated_fn.block = parse_quote! {
        {
            let (__jar, __runtime) = <_ as salsa::storage::HasJar<#jar_ty>>::jar(#db_var);
            let __ingredients = <_ as salsa::storage::HasIngredientsFor<#config_ty>>::ingredient(__jar);
            let __key = __ingredients.intern_map.intern(__runtime, (#(#arg_names),*));
            __ingredients.function.accumulated::<__A>(#db_var, __key)
        }
    };

    Ok(accumulated_fn)
}

/// Examines the function arguments and returns a tuple of:
///
/// * the name of the database argument
/// * the name(s) of the key arguments
fn fn_args(item_fn: &syn::ItemFn) -> syn::Result<(proc_macro2::Ident, Vec<proc_macro2::Ident>)> {
    // Check that we have no receiver and that all arguments have names
    if item_fn.sig.inputs.is_empty() {
        return Err(syn::Error::new(
            item_fn.sig.span(),
            "method needs a database argument",
        ));
    }

    let mut input_names = vec![];
    for input in &item_fn.sig.inputs {
        match input {
            syn::FnArg::Receiver(r) => {
                return Err(syn::Error::new(r.span(), "no self argument expected"));
            }
            syn::FnArg::Typed(pat_ty) => match &*pat_ty.pat {
                syn::Pat::Ident(ident) => {
                    input_names.push(ident.ident.clone());
                }

                _ => {
                    return Err(syn::Error::new(
                        pat_ty.pat.span(),
                        "all arguments must be given names",
                    ));
                }
            },
        }
    }

    // Database is the first argument
    let db_var = input_names[0].clone();
    let arg_names = input_names[1..].to_owned();

    Ok((db_var, arg_names))
}
