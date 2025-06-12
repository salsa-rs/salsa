/// Describes how likely a value is to change—how "durable" it is.
///
/// By default, inputs have `Durability::LOW` and interned values have
/// `Durability::HIGH`. But inputs can be explicitly set with other
/// durabilities.
///
/// We use durabilities to optimize the work of "revalidating" a query
/// after some input has changed. Ordinarily, in a new revision,
/// queries have to trace all their inputs back to the base inputs to
/// determine if any of those inputs have changed. But if we know that
/// the only changes were to inputs of low durability (the common
/// case), and we know that the query only used inputs of medium
/// durability or higher, then we can skip that enumeration.
///
/// Typically, one assigns low durabilities to inputs that the user is
/// frequently editing. Medium or high durabilities are used for
/// configuration, the source from library crates, or other things
/// that are unlikely to be edited.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Durability(DurabilityVal);

#[cfg(feature = "persistence")]
impl serde::Serialize for Durability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serde::Serialize::serialize(&(self.0 as u8), serializer)
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for Durability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        u8::deserialize(deserializer).map(|value| Self(DurabilityVal::from(value)))
    }
}

impl std::fmt::Debug for Durability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            match self.0 {
                DurabilityVal::Low => f.write_str("Durability::LOW"),
                DurabilityVal::Medium => f.write_str("Durability::MEDIUM"),
                DurabilityVal::High => f.write_str("Durability::HIGH"),
                DurabilityVal::NeverChange => f.write_str("Durability::NEVER_CHANGE"),
            }
        } else {
            f.debug_tuple("Durability")
                .field(&(self.0 as usize))
                .finish()
        }
    }
}

// We use an enum here instead of a u8 for niches.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum DurabilityVal {
    Low = 0,
    Medium = 1,
    High = 2,
    NeverChange = 3,
}

impl From<u8> for DurabilityVal {
    #[inline]
    fn from(value: u8) -> Self {
        match value {
            0 => DurabilityVal::Low,
            1 => DurabilityVal::Medium,
            2 => DurabilityVal::High,
            3 => DurabilityVal::NeverChange,
            _ => panic!("invalid durability"),
        }
    }
}

impl Durability {
    /// Low durability: things that change frequently.
    ///
    /// Example: part of the crate being edited
    pub const LOW: Durability = Durability(DurabilityVal::Low);

    /// Medium durability: things that change sometimes, but rarely.
    ///
    /// Example: a Cargo.toml file
    pub const MEDIUM: Durability = Durability(DurabilityVal::Medium);

    /// High durability: things that are not expected to change under
    /// common usage.
    pub const HIGH: Durability = Durability(DurabilityVal::High);

    /// The input is guaranteed to never change. Queries calling it won't have
    /// it as a dependency.
    ///
    /// Example: the standard library or something from crates.io.
    pub const NEVER_CHANGE: Durability = Durability(DurabilityVal::NeverChange);

    /// The minimum possible durability; equivalent to LOW but
    /// "conceptually" distinct (i.e., if we add more durability
    /// levels, this could change).
    pub const MIN: Durability = Self::LOW;

    /// The maximum possible durability; equivalent to NEVER_CHANGE but
    /// "conceptually" distinct (i.e., if we add more durability
    /// levels, this could change).
    pub(crate) const MAX: Durability = Self::NEVER_CHANGE;

    /// Number of durability levels.
    pub(crate) const LEN: usize = Self::MAX.0 as usize + 1;

    #[inline]
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

impl Default for Durability {
    fn default() -> Self {
        Durability::LOW
    }
}
