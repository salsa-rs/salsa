/// Macro for setting up a function that must intern its arguments.
#[macro_export]
macro_rules! setup_tracked_fn {
    (
        // Attributes on the function
        attrs: [$(#[$attr:meta]),*],

        // Visibility of the function
        vis: $vis:vis,

        // Name of the function
        fn_name: $fn_name:ident,

        // Name of the `'db` lifetime that the user gave; if they didn't, then defaults to `'db`
        db_lt: $db_lt:lifetime,

        // Path to the database trait that the user's database parameter used
        Db: $Db:path,

        // Name of the database parameter given by the user.
        db: $db:ident,

        // An identifier for each function argument EXCEPT the database.
        // We prefer to use the identifier the user gave, but if the user gave a pattern
        // (e.g., `(a, b): (u32, u32)`) we will synthesize an identifier.
        input_ids: [$($input_id:ident),*],

        // Types of the function arguments (may reference `$generics`).
        input_tys: [$($input_ty:ty),*],

        // Return type of the function (may reference `$generics`).
        output_ty: $output_ty:ty,

        // Function body, may reference identifiers defined in `$input_pats` and the generics from `$generics`
        inner_fn: {$($inner_fn:tt)*},

        // Path to the cycle recovery function to use.
        cycle_recovery_fn: ($($cycle_recovery_fn:tt)*),

        // Path to function to get the initial value to use for cycle recovery.
        cycle_recovery_initial: ($($cycle_recovery_initial:tt)*),

        // Name of cycle recovery strategy variant to use.
        cycle_recovery_strategy: $cycle_recovery_strategy:ident,

        // If true, this is specifiable.
        is_specifiable: $is_specifiable:tt,

        // If true, don't backdate the value when the new value compares equal to the old value.
        no_eq: $no_eq:tt,

        // If true, the input needs an interner (because it has >1 argument).
        needs_interner: $needs_interner:tt,

        // LRU capacity (a literal, maybe 0)
        lru: $lru:tt,

        // True if we `return_ref` flag was given to the function
        return_ref: $return_ref:tt,

        assert_return_type_is_update: {$($assert_return_type_is_update:tt)*},

        // Annoyingly macro-rules hygiene does not extend to items defined in the macro.
        // We have the procedural macro generate names for those items that are
        // not used elsewhere in the user's code.
        unused_names: [
            $zalsa:ident,
            $Configuration:ident,
            $InternedData:ident,
            $FN_CACHE:ident,
            $INTERN_CACHE:ident,
            $inner:ident,
        ]
    ) => {
        // Suppress this clippy lint because we sometimes require `'db` where the ordinary Rust rules would not.
        #[allow(clippy::needless_lifetimes)]
        $(#[$attr])*
        $vis fn $fn_name<$db_lt>(
            $db: &$db_lt dyn $Db,
            $($input_id: $input_ty,)*
        ) -> salsa::plumbing::macro_if! {
            if $return_ref {
                &$db_lt $output_ty
            } else {
                $output_ty
            }
        } {
            use salsa::plumbing as $zalsa;

            struct $Configuration;

            static $FN_CACHE: $zalsa::IngredientCache<$zalsa::function::IngredientImpl<$Configuration>> =
                $zalsa::IngredientCache::new();

            $zalsa::macro_if! {
                if $needs_interner {
                    #[derive(Copy, Clone)]
                    struct $InternedData<$db_lt>(
                        salsa::Id,
                        std::marker::PhantomData<&$db_lt $zalsa::interned::Value<$Configuration>>,
                    );

                    static $INTERN_CACHE: $zalsa::IngredientCache<$zalsa::interned::IngredientImpl<$Configuration>> =
                        $zalsa::IngredientCache::new();

                    impl $zalsa::SalsaStructInDb for $InternedData<'_> {
                        type MemoIngredientMap = $zalsa::MemoIngredientSingletonIndex;

                        fn lookup_or_create_ingredient_index(aux: &$zalsa::Zalsa) -> $zalsa::IngredientIndices {
                            $zalsa::IngredientIndices::empty()
                        }

                        #[inline]
                        fn cast(id: $zalsa::Id, type_id: ::core::any::TypeId) -> Option<Self> {
                            if type_id == ::core::any::TypeId::of::<$InternedData>() {
                                Some($InternedData(id, ::core::marker::PhantomData))
                            } else {
                                None
                            }
                        }
                    }

                    impl $zalsa::interned::Configuration for $Configuration {
                        const DEBUG_NAME: &'static str = "Configuration";

                        type Fields<$db_lt> = ($($input_ty),*);

                        type Struct<$db_lt> = $InternedData<$db_lt>;

                        fn struct_from_id<$db_lt>(
                            id: salsa::Id,
                        ) -> Self::Struct<$db_lt> {
                            $InternedData(id, std::marker::PhantomData)
                        }

                        fn deref_struct(s: Self::Struct<'_>) -> salsa::Id {
                            s.0
                        }
                    }
                } else {
                    type $InternedData<$db_lt> = ($($input_ty),*);
                }
            }

            impl $Configuration {
                fn fn_ingredient(db: &dyn $Db) -> &$zalsa::function::IngredientImpl<$Configuration> {
                    $FN_CACHE.get_or_create(db.as_dyn_database(), || {
                        <dyn $Db as $Db>::zalsa_register_downcaster(db);
                        db.zalsa().add_or_lookup_jar_by_type::<$Configuration>()
                    })
                }

                pub fn fn_ingredient_mut(db: &mut dyn $Db) -> &mut $zalsa::function::IngredientImpl<Self> {
                    <dyn $Db as $Db>::zalsa_register_downcaster(db);
                    let zalsa_mut = db.zalsa_mut();
                    let index = zalsa_mut.add_or_lookup_jar_by_type::<$Configuration>();
                    let (ingredient, _) = zalsa_mut.lookup_ingredient_mut(index);
                    ingredient.assert_type_mut::<$zalsa::function::IngredientImpl<Self>>()
                }

                $zalsa::macro_if! { $needs_interner =>
                    fn intern_ingredient(
                        db: &dyn $Db,
                    ) -> &$zalsa::interned::IngredientImpl<$Configuration> {
                        $INTERN_CACHE.get_or_create(db.as_dyn_database(), || {
                            <dyn $Db as $Db>::zalsa_register_downcaster(db);
                            db.zalsa().add_or_lookup_jar_by_type::<$Configuration>().successor(0)
                        })
                    }
                }
            }

            impl $zalsa::function::Configuration for $Configuration {
                const DEBUG_NAME: &'static str = stringify!($fn_name);

                type DbView = dyn $Db;

                type SalsaStruct<$db_lt> = $InternedData<$db_lt>;

                type Input<$db_lt> = ($($input_ty),*);

                type Output<$db_lt> = $output_ty;

                const CYCLE_STRATEGY: $zalsa::CycleRecoveryStrategy = $zalsa::CycleRecoveryStrategy::$cycle_recovery_strategy;

                fn values_equal(
                    old_value: &Self::Output<'_>,
                    new_value: &Self::Output<'_>,
                ) -> bool {
                    $zalsa::macro_if! {
                        if $no_eq {
                            false
                        } else {
                            $zalsa::values_equal(old_value, new_value)
                        }
                    }
                }

                fn execute<$db_lt>($db: &$db_lt Self::DbView, ($($input_id),*): ($($input_ty),*)) -> Self::Output<$db_lt> {
                    $($assert_return_type_is_update)*

                    $($inner_fn)*

                    $inner($db, $($input_id),*)
                }

                fn cycle_initial<$db_lt>(db: &$db_lt dyn $Db, ($($input_id),*): ($($input_ty),*)) -> Self::Output<$db_lt> {
                    $($cycle_recovery_initial)*(db, $($input_id),*)
                }

                fn recover_from_cycle<$db_lt>(
                    db: &$db_lt dyn $Db,
                    value: &Self::Output<$db_lt>,
                    count: u32,
                    ($($input_id),*): ($($input_ty),*)
                ) -> $zalsa::CycleRecoveryAction<Self::Output<$db_lt>> {
                    $($cycle_recovery_fn)*(db, value, count, $($input_id),*)
                }

                fn id_to_input<$db_lt>(db: &$db_lt Self::DbView, key: salsa::Id) -> Self::Input<$db_lt> {
                    $zalsa::macro_if! {
                        if $needs_interner {
                            $Configuration::intern_ingredient(db).data(db.as_dyn_database(), key).clone()
                        } else {
                            $zalsa::FromIdWithDb::from_id(key, db)
                        }
                    }
                }
            }

            impl $zalsa::Jar for $Configuration {
                fn create_dependencies(zalsa: &$zalsa::Zalsa) -> $zalsa::IngredientIndices
                where
                    Self: Sized
                {
                    $zalsa::macro_if! {
                        if $needs_interner {
                            $zalsa::IngredientIndices::empty()
                        } else {
                            <$InternedData as $zalsa::SalsaStructInDb>::lookup_or_create_ingredient_index(zalsa)
                        }
                    }
                }

                fn create_ingredients(
                    zalsa: &$zalsa::Zalsa,
                    first_index: $zalsa::IngredientIndex,
                    struct_index: $zalsa::IngredientIndices,
                ) -> Vec<Box<dyn $zalsa::Ingredient>> {
                    let struct_index: $zalsa::IngredientIndices = $zalsa::macro_if! {
                        if $needs_interner {
                            first_index.successor(0).into()
                        } else {
                            struct_index
                        }
                    };

                    $zalsa::macro_if! { $needs_interner =>
                        let intern_ingredient = <$zalsa::interned::IngredientImpl<$Configuration>>::new(
                            first_index.successor(0)
                        );
                    }

                    let intern_ingredient_memo_types = $zalsa::macro_if! {
                        if $needs_interner {
                            Some($zalsa::Ingredient::memo_table_types(&intern_ingredient))
                        } else {
                            None
                        }
                    };
                    // SAFETY: We call with the correct memo types.
                    let memo_ingredient_indices = unsafe {
                        $zalsa::NewMemoIngredientIndices::create(
                            zalsa,
                            struct_index,
                            first_index,
                            $zalsa::function::MemoEntryType::of::<$zalsa::function::Memo<$Configuration>>(),
                            intern_ingredient_memo_types,
                        )
                    };

                    // SAFETY: We pass the MemoEntryType for this Configuration, and we lookup the memo types table correctly.
                    let fn_ingredient = unsafe {
                        <$zalsa::function::IngredientImpl<$Configuration>>::new(
                            first_index,
                            memo_ingredient_indices,
                            $lru,
                            zalsa.views().downcaster_for::<dyn $Db>(),
                        )
                    };
                    $zalsa::macro_if! {
                        if $needs_interner {
                            vec![
                                Box::new(fn_ingredient),
                                Box::new(intern_ingredient),
                            ]
                        } else {
                            vec![
                                Box::new(fn_ingredient),
                            ]
                        }
                    }
                }

                fn id_struct_type_id() -> $zalsa::TypeId {
                    $zalsa::TypeId::of::<$InternedData<'static>>()
                }
            }

            #[allow(non_local_definitions)]
            impl $fn_name {
                pub fn accumulated<$db_lt, A: salsa::Accumulator>(
                    $db: &$db_lt dyn $Db,
                    $($input_id: $input_ty,)*
                ) -> Vec<&$db_lt A> {
                    use salsa::plumbing as $zalsa;
                    let key = $zalsa::macro_if! {
                        if $needs_interner {
                            $Configuration::intern_ingredient($db).intern_id($db.as_dyn_database(), ($($input_id),*), |_, data| data)
                        } else {
                            $zalsa::AsId::as_id(&($($input_id),*))
                        }
                    };

                    $Configuration::fn_ingredient($db).accumulated_by::<A>($db, key)
                }

                $zalsa::macro_if! { $is_specifiable =>
                    pub fn specify<$db_lt>(
                        $db: &$db_lt dyn $Db,
                        $($input_id: $input_ty,)*
                        value: $output_ty,
                    ) {
                        let key = $zalsa::AsId::as_id(&($($input_id),*));
                        $Configuration::fn_ingredient($db).specify_and_record(
                            $db,
                            key,
                            value,
                        )
                    }
                }

                $zalsa::macro_if! { if0 $lru { } else {
                    /// Sets the lru capacity
                    ///
                    /// **WARNING:** Just like an ordinary write, this method triggers
                    /// cancellation. If you invoke it while a snapshot exists, it
                    /// will block until that snapshot is dropped -- if that snapshot
                    /// is owned by the current thread, this could trigger deadlock.
                    #[allow(dead_code)]
                    fn set_lru_capacity(db: &mut dyn $Db, value: usize) {
                        $Configuration::fn_ingredient_mut(db).set_capacity(value);
                    }
                } }
            }

            $zalsa::attach($db, || {
                let result = $zalsa::macro_if! {
                    if $needs_interner {
                        {
                            let key = $Configuration::intern_ingredient($db).intern_id($db.as_dyn_database(), ($($input_id),*), |_, data| data);
                            $Configuration::fn_ingredient($db).fetch($db, key)
                        }
                    } else {
                        $Configuration::fn_ingredient($db).fetch($db, $zalsa::AsId::as_id(&($($input_id),*)))
                    }
                };

                $zalsa::macro_if! {
                    if $return_ref {
                        result
                    } else {
                        <$output_ty as std::clone::Clone>::clone(result)
                    }
                }
            })
        }
        // The struct needs be last in the macro expansion in order to make the tracked
        // function's ident be identified as a function, not a struct, during semantic highlighting.
        // for more details, see https://github.com/salsa-rs/salsa/pull/612.
        #[allow(non_camel_case_types)]
        $vis struct $fn_name {
            _priv: std::convert::Infallible,
        }
    };
}
