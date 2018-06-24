#![deny(missing_docs)]

//! A runtime-borrowchecked arena for storing and referencing values
//!
//! When a value is put into an arena, it will stay there for the whole
//! lifetime of the arena, and never move. So we can safely create multiple
//! mutable references into the arena, while still adding new elements.

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};

static IDS: AtomicUsize = AtomicUsize::new(99);

const MUTABLY: usize = std::usize::MAX;

const BASE_SIZE: usize = 32;
const NUM_ALLOCATIONS: usize = 32;

/// A reference into the arena that can be used for lookup
///
/// An OwnedRef is only valid for the arena that spawned it.
#[derive(Clone, Copy, Debug)]
pub struct OwnedRef {
    arena_index: usize,
    arena_id: usize,
}

/// An immutable reference to a value in the arena.
pub struct ArenaRef<'a, T: 'a> {
    value: &'a T,
    arena_index: usize,
    arena_id: usize,
    borrow: &'a UnsafeCell<usize>,
}

impl<'a, T> ArenaRef<'a, T> {
    /// Throws away the reference and yields an
    /// owned representation of the value.
    pub fn into_owned(self) -> OwnedRef {
        unsafe {
            *self.borrow.get() -= 1;
        }
        OwnedRef {
            arena_id: self.arena_id,
            arena_index: self.arena_index,
        }
    }
}

impl<'a, T> Clone for ArenaRef<'a, T> {
    fn clone(&self) -> Self {
        unsafe {
            *self.borrow.get() += 1;
        }
        ArenaRef {
            value: self.value,
            arena_index: self.arena_index,
            arena_id: self.arena_id,
            borrow: self.borrow,
        }
    }
}

impl<'a, T> Deref for ArenaRef<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value
    }
}

/// A mutable reference to a value in the arena.
impl<'a, T> Deref for ArenaRefMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.value }
    }
}

impl<'a, T> DerefMut for ArenaRefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.value }
    }
}

/// A mutable reference to a value in the arena.
pub struct ArenaRefMut<'a, T> {
    value: *mut T,
    arena_index: usize,
    arena_id: usize,
    borrow: &'a UnsafeCell<usize>,
}

impl<'a, T> ArenaRefMut<'a, T> {
    /// Downgrades the mutable borrow to an immutable one
    pub fn downgrade(self) -> ArenaRef<'a, T> {
        unsafe {
            *self.borrow.get() = 1;
        }
        ArenaRef {
            value: unsafe { &*self.value },
            arena_index: self.arena_index,
            arena_id: self.arena_id,
            borrow: self.borrow,
        }
    }

    /// Throws away the reference and yields an
    /// owned representation of the value.
    pub fn into_owned(self) -> OwnedRef {
        unsafe {
            *self.borrow.get() = 0;
        }
        OwnedRef {
            arena_id: self.arena_id,
            arena_index: self.arena_index,
        }
    }
}

impl<'a, T> Drop for ArenaRefMut<'a, T> {
    fn drop(&mut self) {
        unsafe {
            // if not downgraded, set borrow to 0
            if *self.borrow.get() == MUTABLY {
                *self.borrow.get() = 0;
            }
        }
    }
}

struct Wrapper<T>
where
    T: Sized,
{
    borrow: UnsafeCell<usize>,
    t: T,
}

impl<T> fmt::Debug for Wrapper<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            let borrow = &*self.borrow.get();
            write!(f, "{:?} - {:?}", borrow, self.t)
        }
    }
}

#[derive(Default)]
/// An arena that can hold values of type `T`.
pub struct Arena<T> {
    id: usize,
    len: UnsafeCell<usize>,
    arenas: UnsafeCell<[Vec<Wrapper<T>>; NUM_ALLOCATIONS]>,
}

impl<T> fmt::Debug for Arena<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            let arenas = &*self.arenas.get();
            write!(f, "{:?}", arenas)
        }
    }
}

impl<T> Arena<T> {
    /// Creates a new empty arena.
    pub fn new() -> Self {
        Arena {
            id: IDS.fetch_add(1, Ordering::Relaxed),
            len: UnsafeCell::new(0),
            arenas: Default::default(),
        }
    }

    /// Gets an `ArenaRef` from an `OwnedRef`
    ///
    /// Also does two runtime checks
    /// * that the reference belongs to this arena.
    /// * that the value is not mutably borrowed at the moment.
    ///
    /// Panics otherwise.
    pub fn get(&self, owned: &OwnedRef) -> ArenaRef<T> {
        assert!(
            owned.arena_id == self.id,
            "Reference is invalid for this arena"
        );
        let i = owned.arena_index;
        let (row, col) = Self::index(i);
        let arenas = unsafe { &*self.arenas.get() };
        let wrapper = &arenas[row][col];
        unsafe {
            assert!(
                *wrapper.borrow.get() != MUTABLY,
                "Value already mutably borrowed"
            );
        }
        ArenaRef {
            arena_index: i,
            arena_id: self.id,
            value: &wrapper.t,
            borrow: &wrapper.borrow,
        }
    }

    /// Gets an `ArenaRefMut` from an `OwnedRef`
    ///
    /// Also does two runtime checks
    /// * that the reference belongs to this arena.
    /// * that the value is not already borrowed at the moment.
    ///
    /// Panics otherwise.
    pub fn get_mut(&self, owned: &OwnedRef) -> ArenaRefMut<T> {
        assert!(
            owned.arena_id == self.id,
            "Reference is invalid for this arena"
        );
        let i = owned.arena_index;
        let (row, col) = Self::index(i);
        let arenas = unsafe { &mut *self.arenas.get() };
        let wrapper = &mut arenas[row][col];
        unsafe {
            assert!(*wrapper.borrow.get() == 0, "Value already borrowed");
            *wrapper.borrow.get() = MUTABLY;
        }
        ArenaRefMut {
            arena_index: i,
            arena_id: self.id,
            value: &mut wrapper.t,
            borrow: &wrapper.borrow,
        }
    }

    /// Puts a value into the arena, returning a mutable reference
    pub fn append(&self, t: T) -> ArenaRefMut<T> {
        let i = unsafe { *self.len.get() };
        let (row, col) = Self::index(i);
        if row > 31 {
            panic!("Arena out of space!");
        }
        let arenas = unsafe { &mut *self.arenas.get() };
        if col == 0 {
            arenas[row] = Vec::with_capacity(BASE_SIZE << row);
        }
        arenas[row].push(Wrapper {
            borrow: UnsafeCell::new(MUTABLY),
            t,
        });
        unsafe {
            *self.len.get() += 1;
        }
        let wrapper = &mut arenas[row][col];
        ArenaRefMut {
            arena_index: i,
            arena_id: self.id,
            value: &mut wrapper.t,
            borrow: &wrapper.borrow,
        }
    }

    fn index(mut i: usize) -> (usize, usize) {
        let mut compare = BASE_SIZE;
        let mut allocation = 0;

        loop {
            if compare > i {
                return (allocation, i);
            } else {
                i -= compare;
            }
            compare = compare << 1;
            allocation += 1;
        }
    }

    #[cfg(test)]
    fn _debug_check_lengths(&self) {
        let arenas = unsafe { &mut *self.arenas.get() };

        for i in 0..NUM_ALLOCATIONS {
            assert!(arenas[i].len() <= BASE_SIZE << i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn simple() {
        let arena = Arena::new();
        let mut mut_ref = arena.append(13);

        assert!(*mut_ref == 13);
        *mut_ref += 1;
        assert!(*mut_ref == 14);

        let immut_ref = mut_ref.downgrade();
        assert!(*immut_ref == 14);
    }

    #[test]
    fn should_drop() {
        let rcs: Vec<_> = (0..1024).map(|_| Rc::new(0)).collect();
        {
            let arena = Arena::new();

            for rc in rcs.iter() {
                arena.append(rc.clone());
            }

            arena._debug_check_lengths();

            // drop arena
            for rc in rcs.iter() {
                assert!(Rc::strong_count(rc) == 2);
            }
        }
        for rc in rcs.iter() {
            assert!(Rc::strong_count(rc) == 1);
        }
    }

    #[test]
    #[should_panic]
    fn mutable_aliasing() {
        let arena = Arena::new();

        let owned = arena.append(13).into_owned();

        let _a = arena.get_mut(&owned);
        arena.get_mut(&owned);
    }

    #[test]
    #[should_panic]
    fn mutable_immutable_aliasing() {
        let arena = Arena::new();

        let mut_ref = arena.append(13);

        let owned = mut_ref.into_owned();

        let mut_ref = arena.get_mut(&owned);

        let _downgraded = mut_ref.downgrade();

        // should still panic!
        arena.get_mut(&owned);
    }

    #[test]
    fn multiple_immutable() {
        let arena = Arena::new();

        let owned = arena.append(13).into_owned();

        {
            let _a = arena.get(&owned);
            let _b = arena.get(&owned);
            let _c = arena.get(&owned);
        }
        // a, b, and c dropped, we should be able to get a mutable again.
        let _d = arena.get_mut(&owned);
    }

    #[test]
    #[should_panic]
    fn arena_reference_mixup() {
        let a = Arena::new();
        let b = Arena::new();

        b.append(84);

        let owned = a.append(13).into_owned();
        b.get(&owned);
    }
}
