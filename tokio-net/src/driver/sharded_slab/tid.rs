use super::{
    cfg::{self, CfgPrivate},
    page, Pack,
};
use std::{
    cell::{Cell, UnsafeCell},
    collections::VecDeque,
    fmt,
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

use lazy_static::lazy_static;

/// Uniquely identifies a thread.
// #[repr(transparent)]
#[derive(Hash)]
pub(crate) struct Tid<C> {
    id: usize,
    _not_send: PhantomData<(UnsafeCell<()>, fn(C))>,
}

#[derive(Debug)]
struct Registration(Cell<Option<usize>>);

struct Registry {
    next: AtomicUsize,
    free: Mutex<VecDeque<usize>>,
}

lazy_static! {
    static ref REGISTRY: Registry = Registry {
        next: AtomicUsize::new(0),
        free: Mutex::new(VecDeque::new()),
    };
}
thread_local! {
    static REGISTRATION: Registration = Registration::new();
}

// === impl Tid ===

impl<C: cfg::Config> Pack<C> for Tid<C> {
    const LEN: usize = C::MAX_SHARDS.trailing_zeros() as usize + 1;
    const BITS: usize = cfg::make_mask(Self::LEN);

    type Prev = page::Addr<C>;

    #[inline(always)]
    fn as_usize(&self) -> usize {
        self.id
    }

    #[inline(always)]
    fn from_usize(id: usize) -> Self {
        debug_assert!(id <= Self::BITS);
        Self {
            id,
            _not_send: PhantomData,
        }
    }
}

impl<C: cfg::Config> Tid<C> {
    #[inline]
    pub(crate) fn current() -> Self {
        REGISTRATION
            .try_with(Registration::current)
            .unwrap_or_else(|_| Self::poisoned())
    }

    pub(crate) fn is_current(&self) -> bool {
        REGISTRATION
            .try_with(|r| self == &r.current::<C>())
            .unwrap_or(false)
    }
}

impl<C> Tid<C> {
    #[inline(always)]
    pub(crate) fn new(id: usize) -> Self {
        Self {
            id,
            _not_send: PhantomData,
        }
    }

    #[cold]
    fn poisoned() -> Self {
        Self {
            id: std::usize::MAX,
            _not_send: PhantomData,
        }
    }

    /// Returns true if the local thread ID was accessed while unwinding.
    pub(crate) fn is_poisoned(&self) -> bool {
        self.id == std::usize::MAX
    }
}

impl<C> fmt::Debug for Tid<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_poisoned() {
            f.debug_tuple("Tid")
                .field(&format_args!("<poisoned>"))
                .finish()
        } else {
            f.debug_tuple("Tid")
                .field(&format_args!("{:#x}", self.id))
                .finish()
        }
    }
}

impl<C> PartialEq for Tid<C> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<C> Eq for Tid<C> {}

impl<C: cfg::Config> Clone for Tid<C> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            _not_send: PhantomData,
        }
    }
}

impl<C: cfg::Config> Copy for Tid<C> {}

// === impl Registration ===

impl Registration {
    fn new() -> Self {
        Self(Cell::new(None))
    }

    #[inline(always)]
    fn current<C: cfg::Config>(&self) -> Tid<C> {
        if let Some(tid) = self.0.get().map(Tid::new) {
            tid
        } else {
            self.register()
        }
    }

    #[cold]
    fn register<C: cfg::Config>(&self) -> Tid<C> {
        let id = REGISTRY
            .free
            .lock()
            .ok()
            .and_then(|mut free| {
                if free.len() > 1 {
                    free.pop_front()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| REGISTRY.next.fetch_add(1, Ordering::AcqRel));
        debug_assert!(id <= Tid::<C>::BITS, "thread ID overflow!");
        self.0.set(Some(id));
        Tid::new(id)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(id) = self.0.get() {
            if let Ok(mut free) = REGISTRY.free.lock() {
                free.push_back(id);
            }
        }
    }
}