//! Interior mutability shared by all harts
use core::arch::asm;
use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const NO_OWNER: usize = usize::MAX;

/// Wrap a static data structure inside it so that we are
/// able to access it without any `unsafe`.
///
/// It is a spin lock. The kernel runs with S-mode interrupts disabled,
/// so a holder is never interrupted on its own hart.
///
/// In order to get mutable reference of inner data, call
/// `exclusive_access`.
pub struct UPSafeCell<T> {
    locked: AtomicBool,
    /// hart holding the lock, to catch a second access on the same hart
    owner: AtomicUsize,
    /// inner data
    inner: UnsafeCell<T>,
}

unsafe impl<T> Sync for UPSafeCell<T> {}

impl<T> UPSafeCell<T> {
    /// User is responsible to guarantee that a hart never accesses it
    /// again before releasing its previous access.
    pub const unsafe fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            owner: AtomicUsize::new(NO_OWNER),
            inner: UnsafeCell::new(value),
        }
    }
    /// Spin until no other hart is accessing the data.
    /// Panic if the current hart is already accessing it.
    #[track_caller]
    pub fn exclusive_access(&self) -> UPRefMut<'_, T> {
        let hart = hart_id();
        // only this hart sets the owner to its own id, so the value is never stale
        if self.owner.load(Ordering::Relaxed) == hart {
            panic!(
                "{} is already accessed on hart {}",
                core::any::type_name::<T>(),
                hart
            );
        }
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
        self.owner.store(hart, Ordering::Relaxed);
        UPRefMut { cell: self }
    }
    /// Whether the current hart is accessing the data.
    pub fn accessed_here(&self) -> bool {
        self.owner.load(Ordering::Relaxed) == hart_id()
    }
}

/// Id of the current hart, kept in `tp` by `entry.asm` and `__alltraps`
pub fn hart_id() -> usize {
    let id;
    unsafe { asm!("mv {}, tp", out(reg) id) };
    id
}

/// Exclusive access to the data in a [`UPSafeCell`], released on drop
pub struct UPRefMut<'a, T> {
    cell: &'a UPSafeCell<T>,
}

impl<T> Deref for UPRefMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the lock is held
        unsafe { &*self.cell.inner.get() }
    }
}

impl<T> DerefMut for UPRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the lock is held
        unsafe { &mut *self.cell.inner.get() }
    }
}

impl<T> Drop for UPRefMut<'_, T> {
    fn drop(&mut self) {
        self.cell.owner.store(NO_OWNER, Ordering::Relaxed);
        self.cell.locked.store(false, Ordering::Release);
    }
}
