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

pub struct ExecuteResult<'db, C: Configuration>(pub(super) Option<&'db Memo<'db, C>>);

pub struct FetchCycleContext<'db, C: Configuration> {
    pub(super) ingredient: &'db IngredientImpl<C>,
    pub(super) db: &'db C::DbView,
    pub(super) zalsa: &'db Zalsa,
    pub(super) zalsa_local: &'db ZalsaLocal,
    pub(super) database_key_index: DatabaseKeyIndex,
    pub(super) memo_ingredient_index: MemoIngredientIndex,
}

pub struct FetchCycleResult<'db, C: Configuration>(pub(super) &'db Memo<'db, C>);

pub trait CycleStrategy<C: Configuration>: 'static {
    const RECOVERY_STRATEGY: CycleRecoveryStrategy;

    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C>;

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C>;
}

#[inline]
pub(super) fn recovery_strategy<C: Configuration>() -> CycleRecoveryStrategy {
    <C::CycleStrategy as CycleStrategy<C>>::RECOVERY_STRATEGY
}

impl<C: Configuration> CycleStrategy<C> for Panic {
    const RECOVERY_STRATEGY: CycleRecoveryStrategy = CycleRecoveryStrategy::Panic;

    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C> {
        let ExecuteContext {
            ingredient,
            db,
            claim_guard,
            opt_old_memo,
            memo_ingredient_index,
        } = context;
        ExecuteResult(ingredient.execute_panic(
            db,
            claim_guard,
            opt_old_memo,
            memo_ingredient_index,
        ))
    }

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C> {
        let FetchCycleContext {
            zalsa_local,
            database_key_index,
            ..
        } = context;
        fetch_cold_cycle_panic(zalsa_local, database_key_index)
    }
}

impl<C: Configuration> CycleStrategy<C> for FallbackImmediate {
    const RECOVERY_STRATEGY: CycleRecoveryStrategy = CycleRecoveryStrategy::FallbackImmediate;

    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C> {
        execute_recoverable(context, CyclePolicy::FallbackImmediate)
    }

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C> {
        fetch_cold_cycle_recoverable(context)
    }
}

impl<C: Configuration> CycleStrategy<C> for Fixpoint {
    const RECOVERY_STRATEGY: CycleRecoveryStrategy = CycleRecoveryStrategy::Fixpoint;

    fn execute<'db>(context: ExecuteContext<'db, C>) -> ExecuteResult<'db, C> {
        execute_recoverable(context, CyclePolicy::Fixpoint)
    }

    fn fetch_cold_cycle<'db>(context: FetchCycleContext<'db, C>) -> FetchCycleResult<'db, C> {
        fetch_cold_cycle_recoverable(context)
    }
}

fn execute_recoverable<'db, C: Configuration>(
    context: ExecuteContext<'db, C>,
    policy: CyclePolicy,
) -> ExecuteResult<'db, C> {
    let ExecuteContext {
        ingredient,
        db,
        claim_guard,
        opt_old_memo,
        memo_ingredient_index,
    } = context;
    ExecuteResult(ingredient.execute_cycle(
        db,
        claim_guard,
        opt_old_memo,
        memo_ingredient_index,
        policy,
    ))
}

fn fetch_cold_cycle_recoverable<'db, C: Configuration>(
    context: FetchCycleContext<'db, C>,
) -> FetchCycleResult<'db, C> {
    let FetchCycleContext {
        ingredient,
        db,
        zalsa,
        database_key_index,
        memo_ingredient_index,
        ..
    } = context;
    let mut state = CycleStateImpl::new(ingredient, db);
    let memo = fetch_cold_cycle_recoverable_erased(
        &mut state,
        zalsa,
        database_key_index,
        memo_ingredient_index,
    );
    FetchCycleResult(memo.downcast::<C>())
}
