use crate::runtime::ChangedAt;
use crate::runtime::QueryDescriptorSet;
use crate::runtime::Revision;
use crate::runtime::Runtime;
use crate::runtime::RuntimeId;
use crate::runtime::StampedValue;
use crate::CycleDetected;
use crate::Database;
use crate::QueryDescriptor;
use crate::QueryFunction;
use crate::QueryStorageOps;
use crate::UncheckedMutQueryStorageOps;
use log::debug;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;
use std::marker::PhantomData;

/// Memoized queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub type MemoizedStorage<DB, Q> = DerivedStorage<DB, Q, AlwaysMemoizeValue>;

/// "Dependency" queries just track their dependencies and not the
/// actual value (which they produce on demand). This lessens the
/// storage requirements.
pub type DependencyStorage<DB, Q> = DerivedStorage<DB, Q, NeverMemoizeValue>;

/// "Dependency" queries just track their dependencies and not the
/// actual value (which they produce on demand). This lessens the
/// storage requirements.
pub type VolatileStorage<DB, Q> = DerivedStorage<DB, Q, VolatileValue>;

/// Handles storage where the value is 'derived' by executing a
/// function (in contrast to "inputs").
pub struct DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    map: RwLock<FxHashMap<Q::Key, QueryState<DB, Q>>>,
    policy: PhantomData<MP>,
}

pub trait MemoizationPolicy<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(key: &Q::Key) -> bool;

    fn should_track_inputs(key: &Q::Key) -> bool;
}

pub enum AlwaysMemoizeValue {}
impl<DB, Q> MemoizationPolicy<DB, Q> for AlwaysMemoizeValue
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        true
    }

    fn should_track_inputs(_key: &Q::Key) -> bool {
        true
    }
}

pub enum NeverMemoizeValue {}
impl<DB, Q> MemoizationPolicy<DB, Q> for NeverMemoizeValue
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        false
    }

    fn should_track_inputs(_key: &Q::Key) -> bool {
        true
    }
}

pub enum VolatileValue {}
impl<DB, Q> MemoizationPolicy<DB, Q> for VolatileValue
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        // Why memoize? Well, if the "volatile" value really is
        // constantly changing, we still want to capture its value
        // until the next revision is triggered and ensure it doesn't
        // change -- otherwise the system gets into an inconsistent
        // state where the same query reports back different values.
        true
    }

    fn should_track_inputs(_key: &Q::Key) -> bool {
        false
    }
}

/// Defines the "current state" of query's memoized results.
enum QueryState<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    /// The runtime with the given id is currently computing the
    /// result of this query; if we see this value in the table, it
    /// indeeds a cycle.
    InProgress(RuntimeId),

    /// We have computed the query already, and here is the result.
    Memoized(Memo<DB, Q>),
}

struct Memo<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    /// Last time the value has actually changed.
    /// changed_at can be less than verified_at.
    changed_at: ChangedAt,

    /// The result of the query, if we decide to memoize it.
    value: Option<Q::Value>,

    /// The inputs that went into our query, if we are tracking them.
    inputs: QueryDescriptorSet<DB>,

    /// Last time that we checked our inputs to see if they have
    /// changed. If this is equal to the current revision, then the
    /// value is up to date. If not, we need to check our inputs and
    /// see if any of them have changed since our last check -- if so,
    /// we'll need to re-execute.
    verified_at: Revision,
}

impl<DB, Q, MP> Default for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn default() -> Self {
        DerivedStorage {
            map: RwLock::new(FxHashMap::default()),
            policy: PhantomData,
        }
    }
}

impl<DB, Q, MP> DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn read(
        &self,
        db: &DB,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        let runtime = db.salsa_runtime();

        let _read_lock = runtime.freeze_revision();

        let revision_now = runtime.current_revision();

        debug!(
            "{:?}({:?}): invoked at {:?}",
            Q::default(),
            key,
            revision_now,
        );

        let mut old_value = {
            let map_read = self.map.upgradable_read();
            if let Some(value) = map_read.get(key) {
                match value {
                    QueryState::InProgress(id) => {
                        if *id == runtime.id() {
                            return Err(CycleDetected);
                        } else {
                            unimplemented!();
                        }
                    }
                    QueryState::Memoized(m) => {
                        debug!(
                            "{:?}({:?}): found memoized value verified_at={:?}",
                            Q::default(),
                            key,
                            m.verified_at,
                        );

                        // We've found that the query is definitely up-to-date.
                        // If the value is also memoized, return it.
                        // Otherwise fallback to recomputing the value.
                        if m.verified_at == revision_now {
                            if let Some(value) = &m.value {
                                debug!(
                                    "{:?}({:?}): returning memoized value (changed_at={:?})",
                                    Q::default(),
                                    key,
                                    m.changed_at,
                                );
                                return Ok(StampedValue {
                                    value: value.clone(),
                                    changed_at: m.changed_at,
                                });
                            };
                        }
                    }
                }
            }

            let mut map_write = RwLockUpgradableReadGuard::upgrade(map_read);
            map_write.insert(key.clone(), QueryState::InProgress(runtime.id()))
        };

        // If we have an old-value, it *may* now be stale, since there
        // has been a new revision since the last time we checked. So,
        // first things first, let's walk over each of our previous
        // inputs and check whether they are out of date.
        if let Some(QueryState::Memoized(old_memo)) = &mut old_value {
            if let Some(value) = old_memo.verify_memoized_value(db) {
                debug!("{:?}({:?}): inputs still valid", Q::default(), key);
                // If none of out inputs have changed since the last time we refreshed
                // our value, then our value must still be good. We'll just patch
                // the verified-at date and re-use it.
                old_memo.verified_at = revision_now;
                let changed_at = old_memo.changed_at;

                let mut map_write = self.map.write();
                self.overwrite_placeholder(runtime, &mut map_write, key, old_value.unwrap());
                return Ok(StampedValue { value, changed_at });
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let (mut stamped_value, inputs) = runtime.execute_query_implementation(descriptor, || {
            debug!("{:?}({:?}): executing query", Q::default(), key);

            if !self.should_track_inputs(key) {
                runtime.report_untracked_read();
            }

            Q::execute(db, key.clone())
        });

        // We assume that query is side-effect free -- that is, does
        // not mutate the "inputs" to the query system. Sanity check
        // that assumption here, at least to the best of our ability.
        assert_eq!(
            runtime.current_revision(),
            revision_now,
            "revision altered during query execution",
        );

        // If the new value is equal to the old one, then it didn't
        // really change, even if some of its inputs have. So we can
        // "backdate" its `changed_at` revision to be the same as the
        // old value.
        if let Some(QueryState::Memoized(old_memo)) = &old_value {
            if old_memo.value.as_ref() == Some(&stamped_value.value) {
                assert!(old_memo.changed_at <= stamped_value.changed_at);
                stamped_value.changed_at = old_memo.changed_at;
            }
        }

        {
            let value = if self.should_memoize_value(key) {
                Some(stamped_value.value.clone())
            } else {
                None
            };
            let mut map_write = self.map.write();
            self.overwrite_placeholder(
                runtime,
                &mut map_write,
                key,
                QueryState::Memoized(Memo {
                    changed_at: stamped_value.changed_at,
                    value,
                    inputs,
                    verified_at: revision_now,
                }),
            );
        }

        Ok(stamped_value)
    }

    fn overwrite_placeholder(
        &self,
        runtime: &Runtime<DB>,
        map_write: &mut FxHashMap<Q::Key, QueryState<DB, Q>>,
        key: &Q::Key,
        value: QueryState<DB, Q>,
    ) {
        let old_value = map_write.insert(key.clone(), value);
        assert!(
            match old_value {
                Some(QueryState::InProgress(id)) => id == runtime.id(),
                _ => false,
            },
            "expected in-progress state",
        );
    }

    fn should_memoize_value(&self, key: &Q::Key) -> bool {
        MP::should_memoize_value(key)
    }

    fn should_track_inputs(&self, key: &Q::Key) -> bool {
        MP::should_track_inputs(key)
    }
}

impl<DB, Q, MP> QueryStorageOps<DB, Q> for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn try_fetch(
        &self,
        db: &DB,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        let StampedValue { value, changed_at } = self.read(db, key, &descriptor)?;

        db.salsa_runtime().report_query_read(descriptor, changed_at);

        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        db: &DB,
        revision: Revision,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> bool {
        let runtime = db.salsa_runtime();
        let revision_now = runtime.current_revision();

        debug!(
            "{:?}({:?})::maybe_changed_since(revision={:?}, revision_now={:?})",
            Q::default(),
            key,
            revision,
            revision_now,
        );

        let value = {
            let map_read = self.map.upgradable_read();
            match map_read.get(key) {
                None | Some(QueryState::InProgress(_)) => return true,
                Some(QueryState::Memoized(memo)) => {
                    // If our memo is still up to date, then check if we've
                    // changed since the revision.
                    if memo.verified_at == revision_now {
                        return memo.changed_at.changed_since(revision);
                    }
                    if memo.value.is_some() {
                        // Otherwise, if we cache values, fall back to the full read to compute the result.
                        drop(memo);
                        drop(map_read);
                        return match self.read(db, key, descriptor) {
                            Ok(v) => v.changed_at.changed_since(revision),
                            Err(CycleDetected) => true,
                        };
                    }
                }
            };
            // If, however, we don't cache values, then optimistically
            // try to advance `verified_at` by walking the inputs.
            let mut map_write = RwLockUpgradableReadGuard::upgrade(map_read);
            map_write.insert(key.clone(), QueryState::InProgress(runtime.id()))
        };

        let mut memo = match value {
            Some(QueryState::Memoized(memo)) => memo,
            _ => unreachable!(),
        };

        if memo.verify_inputs(db) {
            memo.verified_at = revision_now;
            self.overwrite_placeholder(
                runtime,
                &mut self.map.write(),
                key,
                QueryState::Memoized(memo),
            );
            return false;
        }

        // Just remove the existing entry. It's out of date.
        self.map.write().remove(key);

        true
    }

    fn is_constant(&self, _db: &DB, key: &Q::Key) -> bool {
        let map_read = self.map.read();
        match map_read.get(key) {
            None => false,
            Some(QueryState::InProgress(_)) => panic!("query in progress"),
            Some(QueryState::Memoized(memo)) => memo.changed_at.is_constant(),
        }
    }
}

impl<DB, Q, MP> UncheckedMutQueryStorageOps<DB, Q> for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn set_unchecked(&self, db: &DB, key: &Q::Key, value: Q::Value) {
        let key = key.clone();

        let mut map_write = self.map.write();

        let current_revision = db.salsa_runtime().current_revision();
        let changed_at = ChangedAt::Revision(current_revision);

        map_write.insert(
            key,
            QueryState::Memoized(Memo {
                value: Some(value),
                changed_at,
                inputs: QueryDescriptorSet::default(),
                verified_at: current_revision,
            }),
        );
    }
}

impl<DB, Q> Memo<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn verify_memoized_value(&self, db: &DB) -> Option<Q::Value> {
        // If we don't have a memoized value, nothing to validate.
        if let Some(v) = &self.value {
            // If inputs are still valid.
            if self.verify_inputs(db) {
                return Some(v.clone());
            }
        }

        None
    }

    fn verify_inputs(&self, db: &DB) -> bool {
        match self.changed_at {
            ChangedAt::Constant(_) => {
                // If we know that the value is constant, it had
                // better not change, but in that case, we ought not
                // to have any inputs. Using `debug_assert` because
                // this is on the fast path.
                debug_assert!(match &self.inputs {
                    QueryDescriptorSet::Tracked(inputs) => inputs.is_empty(),
                    QueryDescriptorSet::Untracked => false,
                });

                true
            }

            ChangedAt::Revision(revision) => match &self.inputs {
                QueryDescriptorSet::Tracked(inputs) => inputs
                    .iter()
                    .all(|old_input| !old_input.maybe_changed_since(db, revision)),

                QueryDescriptorSet::Untracked => false,
            },
        }
    }
}
