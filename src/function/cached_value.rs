use super::Configuration;

/// Storage for an output that can be decoded on demand. Non-persisted queries
/// use an uninhabited type, so their memos require no additional space.
#[doc(hidden)]
pub trait CachedValue<C: Configuration + ?Sized>: std::fmt::Debug + Send + Sync + 'static {
    fn get(&self) -> Option<&C::Output<'static>>;
    fn resident(&self) -> Option<&C::Output<'static>>;
    fn available(&self) -> bool;

    #[cfg(feature = "persistence")]
    fn encoded(&self) -> (&bytes::Bytes, &bytes::Bytes);

    #[cfg(feature = "persistence")]
    fn new(bytes: bytes::Bytes, record: bytes::Bytes, references: bytes::Bytes) -> Self;
}

impl<C: Configuration> CachedValue<C> for std::convert::Infallible {
    fn get(&self) -> Option<&C::Output<'static>> {
        match *self {}
    }
    fn resident(&self) -> Option<&C::Output<'static>> {
        match *self {}
    }
    fn available(&self) -> bool {
        match *self {}
    }

    #[cfg(feature = "persistence")]
    fn encoded(&self) -> (&bytes::Bytes, &bytes::Bytes) {
        match *self {}
    }

    #[cfg(feature = "persistence")]
    fn new(_: bytes::Bytes, _: bytes::Bytes, _: bytes::Bytes) -> Self {
        unreachable!("this query does not persist its value")
    }
}

#[cfg(feature = "persistence")]
#[doc(hidden)]
pub struct LazyValue<C: Configuration>(Box<LazyData<C>>);

#[cfg(feature = "persistence")]
struct LazyData<C: Configuration> {
    bytes: bytes::Bytes,
    record: bytes::Bytes,

    references: bytes::Bytes,
    value: std::sync::OnceLock<Option<C::Output<'static>>>,
}

#[cfg(feature = "persistence")]
impl<C: Configuration> std::fmt::Debug for LazyValue<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyValue")
            .field("bytes", &self.0.bytes.len())
            .finish()
    }
}

#[cfg(feature = "persistence")]
impl<C: Configuration> CachedValue<C> for LazyValue<C> {
    fn new(bytes: bytes::Bytes, record: bytes::Bytes, references: bytes::Bytes) -> Self {
        Self(Box::new(LazyData {
            bytes,
            record,
            references,

            value: std::sync::OnceLock::new(),
        }))
    }

    fn get(&self) -> Option<&C::Output<'static>> {
        self.0
            .value
            .get_or_init(|| {
                let mut decoder = bincode::serde::BorrowedSerdeDecoder::from_slice(
                    &self.0.bytes,
                    bincode::config::standard(),
                    (),
                );
                C::deserialize(decoder.as_deserializer()).ok()
            })
            .as_ref()
    }

    fn resident(&self) -> Option<&C::Output<'static>> {
        self.0.value.get()?.as_ref()
    }

    fn available(&self) -> bool {
        self.0.value.get().is_none_or(Option::is_some)
    }

    fn encoded(&self) -> (&bytes::Bytes, &bytes::Bytes) {
        (&self.0.record, &self.0.references)
    }
}
