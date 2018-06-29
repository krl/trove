#![deny(missing_docs)]

//! A runtime-borrowchecked arena for storing and referencing values
//!
//! When a value is put into an arena, it will stay there for the whole
//! lifetime of the arena, and never move. So we can safely create multiple
//! mutable references into the arena, while still adding new elements.

use std::cell::UnsafeCell;
use std::{fmt, mem};

const BASE: usize = 32;
const NUM_ALLOCATIONS: usize = 32;
const USIZE_BITS: usize = mem::size_of::<usize>() * 8;

/// A reference into the arena that can be used for lookup
#[derive(Clone, Copy, Debug)]
pub struct ArenaRef {
    arena_index: usize,
}

#[derive(Default)]
/// An arena that can hold values of type `T`.
pub struct Arena<T> {
    len: UnsafeCell<usize>,
    arenas: UnsafeCell<[Vec<T>; NUM_ALLOCATIONS]>,
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

impl<T> Clone for Arena<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        unsafe {
            let arenas = (*self.arenas.get()).clone();
            let len = *self.len.get();
            Arena {
                len: UnsafeCell::new(len),
                arenas: UnsafeCell::new(arenas),
            }
        }
    }
}

/// An iterator over all elements in the Arena
pub struct ArenaIter<'a, T: 'a> {
    ofs: usize,
    arena: &'a Arena<T>,
}

impl<'a, T> Iterator for ArenaIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            let len = *self.arena.len.get();
            let index = self.ofs;
            if index == len {
                None
            } else {
                self.ofs += 1;
                Some(self.arena.get(&ArenaRef { arena_index: index }))
            }
        }
    }
}

impl<T> Arena<T> {
    /// Creates a new empty arena.
    pub fn new() -> Self {
        Arena {
            len: UnsafeCell::new(0),
            arenas: Default::default(),
        }
    }

    /// Get a reference into the arena.
    ///
    /// Panics on out-of bound access.
    pub fn get(&self, arena_ref: &ArenaRef) -> &T {
        let i = arena_ref.arena_index;
        if i >= unsafe { *self.len.get() } {
            panic!("Index out of bounds")
        }
        let (row, col) = Self::index(i);
        let arenas = unsafe { &*self.arenas.get() };
        &arenas[row][col]
    }

    /// Get a mutable reference into the arena.
    /// this is unsafe, since you could easily alias mutable references.
    pub unsafe fn get_mut(&self, owned: &ArenaRef) -> &mut T {
        let i = owned.arena_index;
        let (row, col) = Self::index(i);
        let arenas = &mut *self.arenas.get();
        &mut arenas[row][col]
    }

    /// Puts a value into the arena, returning an owned reference
    pub fn append(&self, t: T) -> ArenaRef {
        let i = unsafe { *self.len.get() };
        let (row, col) = Self::index(i);
        if row > 31 {
            panic!("Arena out of space!");
        }
        let arenas = unsafe { &mut *self.arenas.get() };
        if col == 0 {
            // allocate new memory
            arenas[row] = Vec::with_capacity(BASE << row);
        }
        arenas[row].push(t);
        unsafe {
            *self.len.get() += 1;
        }
        ArenaRef { arena_index: i }
    }

    /// Returns an iterator over all elements in the Arena
    pub fn iter(&self) -> ArenaIter<T> {
        ArenaIter {
            arena: self,
            ofs: 0,
        }
    }

    fn index(i: usize) -> (usize, usize) {
        let j = i / BASE + 1;
        let row = USIZE_BITS - j.leading_zeros() as usize - 1;
        (row, i - (2usize.pow(row as u32) -1) * BASE)
    }

    #[cfg(test)]
    fn _debug_check_lengths(&self) {
        let arenas = unsafe { &mut *self.arenas.get() };

        for i in 0..NUM_ALLOCATIONS {
            assert!(arenas[i].len() <= BASE << i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn simple() {
        unsafe {
            let arena = Arena::new();
            let a = arena.append(13);

            assert!(arena.get(&a) == &13);
            *arena.get_mut(&a) += 1;

            assert!(arena.get(&a) == &14);
        }
    }

    #[test]
    fn iter() {
        let arena = Arena::new();

        for i in 0..32 {
            arena.append(i);
        }

        let mut count = 0;

        let mut iter = arena.iter();

        while let Some(i) = iter.next() {
            assert_eq!(i, &count);
            count += 1;
        }
        assert_eq!(count, 32);
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
}
