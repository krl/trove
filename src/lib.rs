#![deny(missing_docs)]
#![allow(unknown_lints)]
#![allow(mut_from_ref)]

//! When a value is put into the Arena, it will stay there for the whole
//! lifetime of the arena, and never move.
//use std::cell::Cell;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::{fmt, mem};

const BASE: usize = 32;
const NUM_ALLOCATIONS: usize = 32;
const USIZE_BITS: usize = mem::size_of::<usize>() * 8;

struct ArenaRefIsNotSend;

/// A reference into the arena that can be used for lookup
/// Also contains a hacky !Send workaround by bundling a
/// `PhantomData<Rc<_>>`
#[derive(Clone, Copy, Debug)]
pub struct ArenaRef((usize, PhantomData<Rc<ArenaRefIsNotSend>>));

/// An arena that can hold values of type `T`.
pub struct Arena<T> {
    len: UnsafeCell<usize>,
    arenas: UnsafeCell<[Vec<T>; NUM_ALLOCATIONS]>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Arena {
            len: Default::default(),
            arenas: Default::default(),
        }
    }
}

impl<T> fmt::Debug for Arena<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[")?;
        let len = unsafe { *self.len.get() };
        for i in 0..len.saturating_sub(1) {
            write!(f, "{:?}, ", self.get(&ArenaRef((i, PhantomData))))?;
        }
        if len > 0 {
            write!(f, "{:?}, ", self.get(&ArenaRef((len - 1, PhantomData))))?;
        }
        write!(f, "]")
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
                Some(self.arena.get(&ArenaRef((index, PhantomData))))
            }
        }
    }
}

impl<T> Arena<T> {
    /// Creates a new empty arena.
    pub fn new() -> Self {
        Arena::default()
    }

    /// Get a reference into the arena.
    ///
    /// Panics on out-of bound access.
    pub fn get(&self, arena_ref: &ArenaRef) -> &T {
        let i = (arena_ref.0).0;
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
        let i = (owned.0).0;
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
        ArenaRef((i, PhantomData))
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
        (row, i - (2usize.pow(row as u32) - 1) * BASE)
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

            for rc in rcs.iter() {
                assert!(Rc::strong_count(rc) == 2);
            }
            // drop arena
        }
        for rc in rcs.iter() {
            assert!(Rc::strong_count(rc) == 1);
        }
    }

    #[test]
    fn readme_example() {
        let arena = Arena::new();

        let a = arena.append(0);
        let b = arena.append(1);

        assert_eq!(*arena.get(&a), 0);

        unsafe {
            *arena.get_mut(&b) += 1;
        }

        assert_eq!(*arena.get(&b), 2);
    }
}
