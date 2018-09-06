#![deny(missing_docs)]
#![allow(unknown_lints)]
#![allow(mut_from_ref)]

//! When a value is put into the Arena, it will stay there for the whole
//! lifetime of the arena, and never move.
//use std::cell::Cell;
use std::cell::{Ref, RefCell, RefMut, UnsafeCell};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::{fmt, mem};

const BASE: usize = 32;
const NUM_ALLOCATIONS: usize = 32;
const USIZE_BITS: usize = mem::size_of::<usize>() * 8;

struct ArenaIdxIsNotSend;

/// A reference into the arena that can be used for lookup
/// Also contains a hacky !Send workaround by bundling a
/// `PhantomData<Rc<_>>`
#[derive(Clone, Copy, Debug)]
pub struct ArenaIdx(usize, PhantomData<Rc<ArenaIdxIsNotSend>>);

#[derive(Debug)]
/// An immutable reference into the arena
pub struct ArenaRef<'a, T: 'a>(Ref<'a, T>);

#[derive(Debug)]
/// A mutable reference into the arena
pub struct ArenaRefMut<'a, T: 'a>(RefMut<'a, T>);

impl<'a, T> Deref for ArenaRef<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &*self.0
    }
}

impl<'a, T> Deref for ArenaRefMut<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &*self.0
    }
}

impl<'a, T> DerefMut for ArenaRefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.0
    }
}

/// An arena that can hold values of type `T`.
pub struct Arena<T> {
    len: UnsafeCell<usize>,
    arenas: UnsafeCell<[Vec<RefCell<T>>; NUM_ALLOCATIONS]>,
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
            write!(f, "{:?}, ", self.get(&ArenaIdx(i, PhantomData)))?;
        }
        if len > 0 {
            write!(f, "{:?}, ", self.get(&ArenaIdx(len - 1, PhantomData)))?;
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
    type Item = ArenaRef<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        let len = unsafe { *self.arena.len.get() };
        let index = self.ofs;
        if index == len {
            None
        } else {
            self.ofs += 1;
            Some(self.arena.get(&ArenaIdx(index, PhantomData)))
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
    pub fn get(&self, arena_ref: &ArenaIdx) -> ArenaRef<T> {
        let i = arena_ref.0;
        if i >= unsafe { *self.len.get() } {
            panic!("Index out of bounds")
        }
        let (row, col) = Self::index(i);
        let arenas = unsafe { &*self.arenas.get() };
        ArenaRef(arenas[row][col].borrow())
    }

    /// Get a mutable reference into the arena.
    /// Panics if aliased, through a `RefCell` wrapper
    pub fn get_mut(&self, owned: &ArenaIdx) -> ArenaRefMut<T> {
        let i = owned.0;
        let (row, col) = Self::index(i);
        let arenas = unsafe { &mut *self.arenas.get() };
        ArenaRefMut(arenas[row][col].borrow_mut())
    }

    /// Puts a value into the arena, returning an index
    pub fn append(&self, t: T) -> ArenaIdx {
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
        arenas[row].push(RefCell::new(t));
        unsafe {
            *self.len.get() += 1;
        }
        ArenaIdx(i, PhantomData)
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
        let arena = Arena::new();
        let a = arena.append(13);

        assert!(*arena.get(&a) == 13);
        *arena.get_mut(&a) += 1;

        assert!(*arena.get(&a) == 14);
    }

    #[test]
    #[should_panic]
    fn mutable_aliasing() {
        let arena = Arena::new();
        let a = arena.append(13);

        let _ref_a = arena.get_mut(&a);
        let _ref_b = arena.get_mut(&a);
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
            assert_eq!(*i, count);
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
        *arena.get_mut(&b) += 1;

        assert_eq!(*arena.get(&b), 2);
    }
}
