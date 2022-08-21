use proc_macro2::{Literal, TokenStream};
use syn::spanned::Spanned;
use syn::{ReturnType, Token};

use crate::configuration::{self, Configuration, CycleRecoveryStrategy};
use crate::options::Options;

pub(crate) fn tracked(
    args: proc_macro::TokenStream,
    item_fn: syn::ItemFn,
) -> proc_macro::TokenStream {
    let args = syn::parse_macro_input!(args as Args);
    match tracked_fn(args, item_fn) {
        Ok(p) => p.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

fn tracked_fn(args: Args, item_fn: syn::ItemFn) -> syn::Result<TokenStream> {
    if item_fn.sig.inputs.len() <= 1 {
        return Err(syn::Error::new(
            item_fn.sig.ident.span(),
            "tracked functions must have at least a database and salsa struct argument",
        ));
    }

    if let Some(s) = &args.specify {
        if requires_interning(&item_fn) {
            return Err(syn::Error::new(
                s.span(),
                "tracked functon takes too many argments to have its value set with `specify`",
            ));
        }

        if args.lru.is_some() {
            return Err(syn::Error::new(
                s.span(),
                "`specify` and `lru` cannot be used together",
            ));
        }
    }

    let struct_item = configuration_struct(&item_fn);
    let configuration = fn_configuration(&args, &item_fn);
    let struct_item_ident = &struct_item.ident;
    let config_ty: syn::Type = parse_quote!(#struct_item_ident);
    let configuration_impl = configuration.to_impl(&config_ty);
    let ingredients_for_impl = ingredients_for_impl(&args, &item_fn, &config_ty);
    let (getter, item_impl) = wrapper_fns(&args, &item_fn, &config_ty)?;

    Ok(quote! {
        #struct_item
        #configuration_impl
        #ingredients_for_impl

        // we generate a `'db` lifetime that clippy
        // sometimes doesn't like
        #[allow(clippy::needless_lifetimes)]
        #getter
        #item_impl
    })
}

type Args = Options<TrackedFn>;

struct TrackedFn;

impl crate::options::AllowedOptions for TrackedFn {
    const RETURN_REF: bool = true;

    const SPECIFY: bool = true;

    const NO_EQ: bool = true;

    const JAR: bool = true;

    const DATA: bool = false;

    const DB: bool = false;

    const RECOVERY_FN: bool = true;

    const LRU: bool = true;

    const CONSTRUCTOR_NAME: bool = false;
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
    let intern_map: syn::Type = if requires_interning(item_fn) {
        let key_ty = key_tuple_ty(item_fn);
        parse_quote! { salsa::interned::InternedIngredient<salsa::Id, #key_ty> }
    } else {
        parse_quote! { salsa::interned::IdentityInterner<#salsa_struct_ty> }
    };

    parse_quote! {
        #[allow(non_camel_case_types)]
        #visibility struct #fn_name {
            intern_map: #intern_map,
            function: salsa::function::FunctionIngredient<Self>,
        }
    }
}

/// True if this fn takes more arguments.
fn requires_interning(item_fn: &syn::ItemFn) -> bool {
    item_fn.sig.inputs.len() > 2
}

/// Every tracked fn takes a salsa struct as its second argument.
/// This fn returns the type of that second argument.
fn salsa_struct_ty(item_fn: &syn::ItemFn) -> &syn::Type {
    match &item_fn.sig.inputs[1] {
        syn::FnArg::Receiver(_) => panic!("receiver not expected"),
        syn::FnArg::Typed(pat_ty) => &pat_ty.ty,
    }
}

fn fn_configuration(args: &Args, item_fn: &syn::ItemFn) -> Configuration {
    let jar_ty = args.jar_ty();
    let salsa_struct_ty = salsa_struct_ty(item_fn).clone();
    let key_ty = if requires_interning(item_fn) {
        parse_quote!(salsa::id::Id)
    } else {
        salsa_struct_ty.clone()
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
    args: &Args,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::ItemImpl {
    let jar_ty = args.jar_ty();

    let intern_map: syn::Expr = if requires_interning(item_fn) {
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
                salsa::interned::InternedIngredient::new(index)
            }
        }
    } else {
        parse_quote! {
            salsa::interned::IdentityInterner::new()
        }
    };

    // set 0 as default to disable LRU
    let lru = args.lru.unwrap_or(0);

    // get the name of the function as a string literal
    let debug_name = Literal::string(&format!("{}", item_fn.sig.ident));

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

fn wrapper_fns(
    args: &Args,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<(syn::ItemFn, syn::ItemImpl)> {
    // The "getter" has same signature as the original:
    let getter_fn = getter_fn(args, item_fn, config_ty)?;

    let ref_getter_fn = ref_getter_fn(args, item_fn, config_ty)?;
    let accumulated_fn = accumulated_fn(args, item_fn, config_ty)?;
    let setter_fn = setter_fn(args, item_fn, config_ty)?;
    let specify_fn = specify_fn(args, item_fn, config_ty)?.map(|f| quote! { #f });

    let setter_impl: syn::ItemImpl = parse_quote! {
        impl #config_ty {
            #[allow(dead_code, clippy::needless_lifetimes)]
            #ref_getter_fn

            #[allow(dead_code, clippy::needless_lifetimes)]
            #setter_fn

            #[allow(dead_code, clippy::needless_lifetimes)]
            #accumulated_fn

            #specify_fn
        }
    };

    Ok((getter_fn, setter_impl))
}

/// Creates the `get` associated function.
fn getter_fn(
    args: &Args,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<syn::ItemFn> {
    let mut getter_fn = item_fn.clone();
    let arg_idents: Vec<_> = item_fn
        .sig
        .inputs
        .iter()
        .map(|arg| -> syn::Result<syn::Ident> {
            match arg {
                syn::FnArg::Receiver(_) => Err(syn::Error::new(arg.span(), "unexpected receiver")),
                syn::FnArg::Typed(pat_ty) => Ok(match &*pat_ty.pat {
                    syn::Pat::Ident(ident) => ident.ident.clone(),
                    _ => return Err(syn::Error::new(arg.span(), "unexpected receiver")),
                }),
            }
        })
        .collect::<Result<_, _>>()?;
    if args.return_ref.is_some() {
        getter_fn = make_fn_return_ref(getter_fn)?;
        getter_fn.block = Box::new(parse_quote_spanned! {
            item_fn.block.span() => {
                #config_ty::get(#(#arg_idents,)*)
            }
        });
    } else {
        getter_fn.block = Box::new(parse_quote_spanned! {
            item_fn.block.span() => {
                Clone::clone(#config_ty::get(#(#arg_idents,)*))
            }
        });
    }
    Ok(getter_fn)
}

/// Creates a `get` associated function that returns `&Value`
/// (to be used when `return_ref` is specified).
///
/// (Helper for `getter_fn`)
fn ref_getter_fn(
    args: &Args,
    item_fn: &syn::ItemFn,
    config_ty: &syn::Type,
) -> syn::Result<syn::ItemFn> {
    let jar_ty = args.jar_ty();
    let mut ref_getter_fn = item_fn.clone();
    ref_getter_fn.sig.ident = syn::Ident::new("get", item_fn.sig.ident.span());
    ref_getter_fn = make_fn_return_ref(ref_getter_fn)?;

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
    args: &Args,
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

fn specify_fn(
    args: &Args,
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
/// Given a function def tagged with `#[return_ref]`, modifies `ref_getter_fn`
/// so that it returns an `&Value` instead of `Value`. May introduce a name for the
/// database lifetime if required.
fn make_fn_return_ref(mut ref_getter_fn: syn::ItemFn) -> syn::Result<syn::ItemFn> {
    // The 0th input should be a `&dyn Foo`. We need to ensure
    // it has a named lifetime parameter.
    let (db_lifetime, _) = db_lifetime_and_ty(&mut ref_getter_fn)?;

    let (right_arrow, elem) = match ref_getter_fn.sig.output {
        ReturnType::Default => (
            syn::Token![->](ref_getter_fn.sig.paren_token.span),
            parse_quote!(()),
        ),
        ReturnType::Type(rarrow, ty) => (rarrow, ty),
    };

    let ref_output = syn::TypeReference {
        and_token: syn::Token![&](right_arrow.span()),
        lifetime: Some(db_lifetime),
        mutability: None,
        elem,
    };

    ref_getter_fn.sig.output = syn::ReturnType::Type(right_arrow, Box::new(ref_output.into()));

    Ok(ref_getter_fn)
}

/// Given an item function, identifies the name given to the `&dyn Db` reference and returns it,
/// along with the type of the database. If the database lifetime did not have a name,
/// then modifies the item function so that it is called `'__db` and returns that.
fn db_lifetime_and_ty(func: &mut syn::ItemFn) -> syn::Result<(syn::Lifetime, &syn::Type)> {
    match &mut func.sig.inputs[0] {
        syn::FnArg::Receiver(r) => Err(syn::Error::new(r.span(), "expected database, not self")),
        syn::FnArg::Typed(pat_ty) => match &mut *pat_ty.ty {
            syn::Type::Reference(ty) => match &ty.lifetime {
                Some(lt) => Ok((lt.clone(), &pat_ty.ty)),
                None => {
                    let and_token_span = ty.and_token.span();
                    let ident = syn::Ident::new("__db", and_token_span);
                    func.sig.generics.params.insert(
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
    args: &Args,
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

    let (db_lifetime, _) = db_lifetime_and_ty(&mut accumulated_fn)?;
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
    // Check that we have no receiver and that all argments have names
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
