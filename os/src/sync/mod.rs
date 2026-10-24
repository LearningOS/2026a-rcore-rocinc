//! Synchronization and interior mutability primitives

mod up;

pub use up::{hart_id, UPRefMut, UPSafeCell};
