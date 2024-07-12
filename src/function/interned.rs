//! Helper code for tracked functions that take arbitrary arguments.
//! These arguments must be interned to create a salsa id before the
//! salsa machinery can execute.

use std::{any::Any, fmt, hash::Hash, marker::PhantomData};

use crate::{
    function, interned, plumbing::CycleRecoveryStrategy, salsa_struct::SalsaStructInDb, Cycle, Id,
};

pub trait Configuration: Any + Copy {
    const DEBUG_NAME: &'static str;
    type DbView: ?Sized + crate::Database;
    type SalsaStruct<'db>: SalsaStructInDb<Self::DbView>;
    type Input<'db>: Send + Sync + Clone + Hash + Eq;
    type Output<'db>: fmt::Debug + Send + Sync;
    const CYCLE_STRATEGY: CycleRecoveryStrategy;
    fn should_backdate_value(old_value: &Self::Output<'_>, new_value: &Self::Output<'_>) -> bool;
    fn id_to_input<'db>(db: &'db Self::DbView, key: Id) -> Self::Input<'db>;
    fn execute<'db>(db: &'db Self::DbView, input: Self::Input<'db>) -> Self::Output<'db>;
    fn recover_from_cycle<'db>(db: &'db Self::DbView, cycle: &Cycle, key: Id) -> Self::Output<'db>;
}

pub struct InterningConfiguration<C: Configuration> {
    phantom: PhantomData<C>,
}

#[derive(Copy, Clone)]
pub struct InternedData<'db, C: Configuration>(
    std::ptr::NonNull<interned::ValueStruct<C>>,
    std::marker::PhantomData<&'db interned::ValueStruct<C>>,
);

impl<C: Configuration> SalsaStructInDb<C::DbView> for InternedData<'_, C> {
    fn register_dependent_fn(_db: &C::DbView, _index: crate::storage::IngredientIndex) {}
}

impl<C: Configuration> interned::Configuration for C {
    const DEBUG_NAME: &'static str = C::DEBUG_NAME;

    type Data<'db> = C::Input<'db>;

    type Struct<'db> = InternedData<'db, C>;

    unsafe fn struct_from_raw<'db>(
        ptr: std::ptr::NonNull<interned::ValueStruct<Self>>,
    ) -> Self::Struct<'db> {
        InternedData(ptr, std::marker::PhantomData)
    }

    fn deref_struct(s: Self::Struct<'_>) -> &interned::ValueStruct<Self> {
        unsafe { s.0.as_ref() }
    }
}

impl<C: Configuration> function::Configuration for C {
    const DEBUG_NAME: &'static str = C::DEBUG_NAME;

    type DbView = C::DbView;

    type SalsaStruct<'db> = InternedData<'db, C>;

    type Input<'db> = C::Input<'db>;

    type Output<'db> = C::Output<'db>;

    const CYCLE_STRATEGY: crate::plumbing::CycleRecoveryStrategy = C::CYCLE_STRATEGY;

    fn should_backdate_value(old_value: &Self::Output<'_>, new_value: &Self::Output<'_>) -> bool {
        C::should_backdate_value(old_value, new_value)
    }

    fn id_to_input<'db>(db: &'db Self::DbView, key: crate::Id) -> Self::Input<'db> {
        todo!()
    }

    fn execute<'db>(db: &'db Self::DbView, input: Self::Input<'db>) -> Self::Output<'db> {
        todo!()
    }

    fn recover_from_cycle<'db>(
        db: &'db Self::DbView,
        cycle: &crate::Cycle,
        key: crate::Id,
    ) -> Self::Output<'db> {
        todo!()
    }
}
