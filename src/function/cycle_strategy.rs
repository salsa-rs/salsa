use super::execute::{CyclePolicy, CycleStateImpl};
use super::fetch::{fetch_cold_cycle_panic, fetch_cold_cycle_recoverable_erased};
use super::memo::Memo;
use super::{ClaimGuard, Configuration, IngredientImpl};
use crate::DatabaseKeyIndex;
use crate::cycle::CycleRecoveryStrategy;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::ZalsaLocal;

pub struct Panic;
pub struct FallbackImmediate;
pub struct Fixpoint;

pub struct ExecuteContext<'db, C: Configuration> {
    pub(super) ingredient: &'db IngredientImpl<C>,
    pub(super) db: &'db C::DbView,
    pub(super) claim_guard: ClaimGuard<'db>,
    pub(super) opt_old_memo: Option<&'db Memo<'db, C>>,
    pub(super) memo_ingredient_index: MemoIngredientIndex,
}

pub type ExecuteResult<'db, C> = Option<&'db Memo<'db, C>>;

pub struct FetchCycleContext<'db, C: Configuration> {
    pub(super) ingredient: &'db IngredientImpl<C>,
    pub(super) db: &'db C::DbView,
    pub(super) zalsa: &'db Zalsa,
    pub(super) zalsa_local: &'db ZalsaLocal,
    pub(super) database_key_index: DatabaseKeyIndex,
    pub(super) memo_ingredient_index: MemoIngredientIndex,
}

pub type FetchCycleResult<'db, C> = &'db Memo<'db, C>;

pub trait CycleStrategy<C: Configuration>: 'static {
    const RECOVERY_STRATEGY: CycleRecoveryStrategy = CycleRecoveryStrategy::Panic;

    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C>;

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C>;
}

impl<C: Configuration> CycleStrategy<C> for Panic {
    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C> {
        IngredientImpl::execute_panic(context)
    }

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C> {
        fetch_cold_cycle_panic(context.zalsa_local, context.database_key_index)
    }
}

impl<C: Configuration> CycleStrategy<C> for FallbackImmediate {
    const RECOVERY_STRATEGY: CycleRecoveryStrategy = CycleRecoveryStrategy::FallbackImmediate;

    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C> {
        IngredientImpl::execute_cycle(context, CyclePolicy::FallbackImmediate)
    }

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C> {
        fetch_cold_cycle_recoverable(context)
    }
}

impl<C: Configuration> CycleStrategy<C> for Fixpoint {
    const RECOVERY_STRATEGY: CycleRecoveryStrategy = CycleRecoveryStrategy::Fixpoint;

    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C> {
        IngredientImpl::execute_cycle(context, CyclePolicy::Fixpoint)
    }

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C> {
        fetch_cold_cycle_recoverable(context)
    }
}

fn fetch_cold_cycle_recoverable<'db, C: Configuration>(
    context: FetchCycleContext<'db, C>,
) -> FetchCycleResult<'db, C> {
    let mut state = CycleStateImpl::new(
        context.ingredient,
        context.db,
        context.memo_ingredient_index,
    );
    fetch_cold_cycle_recoverable_erased(&mut state, context.zalsa, context.database_key_index)
        .downcast::<C>()
}
