#![deny(missing_docs)]
#![allow(unknown_lints)]
#![allow(mut_from_ref)]

//! Thread-local clonable arena allocator
extern crate either;
extern crate vec_map;

use std::cell::{
    BorrowError, BorrowMutError, Ref, RefCell, RefMut, UnsafeCell,
};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::{fmt, mem};

use either::Either;
use vec_map::VecMap;

const BASE: usize = 32;
const NUM_ALLOCATIONS: usize = 32;
const USIZE_BITS: usize = mem::size_of::<usize>() * 8;

struct ArenaIdxIsNotSend;

thread_local! {
    static IDS: RefCell<usize> = RefCell::new(0);
}

/// A reference into the arena that can be used for lookup
/// Also contains a hacky !Send workaround by bundling a
/// `PhantomData<Rc<_>>`
#[derive(Clone, Copy, Debug)]
pub struct ArenaIdx {
    arena: usize,
    offset: usize,
    _marker: PhantomData<Rc<ArenaIdxIsNotSend>>,
}

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

struct ArenaInner<T> {
    rows: UnsafeCell<[Vec<RefCell<T>>; NUM_ALLOCATIONS]>,
    len: RefCell<usize>,
}

impl<T> Default for ArenaInner<T> {
    fn default() -> Self {
        ArenaInner {
            rows: UnsafeCell::new(Default::default()),
            len: RefCell::new(0),
        }
    }
}

/// An arena that can hold values of type `T`.
pub struct Arena<T> {
    id: RefCell<usize>,
    arenas: UnsafeCell<VecMap<Rc<ArenaInner<T>>>>,
}

impl<T> Clone for Arena<T> {
    fn clone(&self) -> Self {
        let new_id_a = new_id();
        let new_id_b = new_id();
        let inner_a = ArenaInner::default();
        let inner_b = ArenaInner::default();

        *self.id.borrow_mut() = new_id_a;
        unsafe {
            let arenas = &mut *self.arenas.get();
            arenas.insert(new_id_a, Rc::new(inner_a));
            arenas.insert(new_id_b, Rc::new(inner_b));
            Arena {
                id: RefCell::new(new_id_b),
                arenas: UnsafeCell::new(arenas.clone()),
            }
        }
    }
}

fn new_id() -> usize {
    IDS.with(|ids| {
        let id = *ids.borrow();
        *ids.borrow_mut() += 1;
        id
    })
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        let mut map = VecMap::new();
        let id = new_id();
        map.insert(id, Default::default());
        Arena {
            id: RefCell::new(id),
            arenas: UnsafeCell::new(map),
        }
    }
}

impl<T: Clone> Arena<T> {
    /// Creates a new empty arena.
    pub fn new() -> Self {
        Arena::default()
    }

    /// Merges two Arenas into one, in which all
    /// nodes reachable from `a` and `b` are reachable
    pub fn merge(a: &Self, b: &Self) -> Self {
        unsafe {
            let arenas_a = &*a.arenas.get();
            let arenas_b = &*b.arenas.get();

            let mut new_arenas = VecMap::new();
            let from_a = arenas_a.iter().map(|(k, v)| (k.clone(), v.clone()));
            let from_b = arenas_b.iter().map(|(k, v)| (k.clone(), v.clone()));
            new_arenas.extend(from_a);
            new_arenas.extend(from_b);

            Arena {
                id: RefCell::new(new_id()),
                arenas: UnsafeCell::new(new_arenas),
            }
        }
    }

    /// Get a reference into the arena.
    ///
    /// Panics on out-of bound access.
    pub fn get(&self, arena_idx: &ArenaIdx) -> ArenaRef<T> {
        self.try_get(arena_idx).unwrap()
    }

    /// Try to get a reference into the arena.
    ///
    /// Returns an error if value cannot be borrowed
    pub fn try_get(
        &self,
        arena_idx: &ArenaIdx,
    ) -> Result<ArenaRef<T>, BorrowError> {
        let arenas = unsafe { &mut *self.arenas.get() };
        arenas
            .get(arena_idx.arena)
            .expect("Invalid arena_idx")
            .try_get(arena_idx.offset)
    }

    /// Get a mutable reference into the arena.
    ///
    /// Panics if aliased, through a `RefCell` wrapper
    pub fn get_mut(&self, arena_idx: &mut ArenaIdx) -> ArenaRefMut<T> {
        self.try_get_mut(arena_idx).unwrap()
    }

    /// Try to get a mutable reference into the arena.
    ///
    /// Returns an error if value cannot be borrowed
    pub fn try_get_mut(
        &self,
        arena_idx: &mut ArenaIdx,
    ) -> Result<ArenaRefMut<T>, Either<BorrowMutError, BorrowError>> {
        let arenas = unsafe { &mut *self.arenas.get() };
        let id = *self.id.borrow();
        if arena_idx.arena == id {
            arenas
                .get(arena_idx.arena)
                .expect("Invalid arena_idx")
                .try_get_mut(arena_idx.offset)
                .map_err(|e| Either::Left(e))
        } else {
            let t: T =
                (*self.try_get(arena_idx).map_err(|e| Either::Right(e))?)
                    .clone();
            *arena_idx = self.append(t);
            self.try_get_mut(arena_idx)
        }
    }

    /// Puts a value into the arena, returning an index
    pub fn append(&self, t: T) -> ArenaIdx {
        let arenas = unsafe { &mut *self.arenas.get() };
        let id = *self.id.borrow();
        arenas.get(id).expect("Invalid arena_idx").append(id, t)
    }
}

impl<T> fmt::Debug for Arena<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        unsafe { write!(f, "{:?}", *self.arenas.get()) }
    }
}

impl<T> ArenaInner<T> {
    fn try_get(&self, offset: usize) -> Result<ArenaRef<T>, BorrowError> {
        if offset >= *self.len.borrow() {
            panic!("Index out of bounds")
        }
        let (row, col) = Self::index(offset);
        unsafe {
            let rows = self.rows.get();
            Ok(ArenaRef((*rows)[row][col].try_borrow()?))
        }
    }

    pub fn try_get_mut(
        &self,
        offset: usize,
    ) -> Result<ArenaRefMut<T>, BorrowMutError> {
        if offset >= *self.len.borrow() {
            panic!("Index out of bounds")
        }
        let (row, col) = Self::index(offset);
        unsafe {
            let rows = &mut *self.rows.get();
            Ok(ArenaRefMut(rows[row][col].try_borrow_mut()?))
        }
    }

    // [0, 1]
    // [2, 3, 4, 5]
    // [6, 7, 8, 9, 10, 11, 12, 13]
    fn index(i: usize) -> (usize, usize) {
        let j = i / BASE + 1;
        let row = USIZE_BITS - j.leading_zeros() as usize - 1;
        (row, i - (2usize.pow(row as u32) - 1) * BASE)
    }

    pub fn append(&self, id: usize, t: T) -> ArenaIdx {
        let i = *self.len.borrow();
        let (row, col) = Self::index(i);
        if row > 31 {
            panic!("Arena out of space!");
        }
        let rows = unsafe { &mut *self.rows.get() };
        if col == 0 {
            // allocate new memory
            rows[row] = Vec::with_capacity(BASE << row);
        }
        rows[row].push(RefCell::new(t));
        *self.len.borrow_mut() += 1;

        ArenaIdx {
            offset: i,
            arena: id,
            _marker: PhantomData,
        }
    }

    fn debug(&self, offset: usize, f: &mut fmt::Formatter) -> fmt::Result
    where
        T: fmt::Debug,
    {
        if offset >= *self.len.borrow() {
            panic!("Index out of bounds")
        }
        let (row, col) = Self::index(offset);
        unsafe {
            let rows = self.rows.get();
            let inner = &*(*rows)[row][col].as_ptr();
            write!(f, "{:?}", inner)
        }
    }
}

impl<T> fmt::Debug for ArenaInner<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let len = *self.len.borrow();
        write!(f, "[")?;
        if len > 0 {
            self.debug(0, f)?;
            for i in 1..len {
                write!(f, ", ")?;
                self.debug(i, f)?
            }
        }
        write!(f, "]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn simple() {
        let arena = Arena::new();
        let mut a = arena.append(13);

        assert!(*arena.get(&a) == 13);
        *arena.get_mut(&mut a) += 1;

        assert!(*arena.get(&a) == 14);
    }

    #[test]
    #[should_panic]
    fn mutable_aliasing_panics() {
        let arena = Arena::new();
        let mut a = arena.append(13);

        let _ref_a = arena.get_mut(&mut a);
        let _ref_b = arena.get_mut(&mut a);
    }

    #[test]
    fn try_mutable_aliasing() {
        let arena = Arena::new();
        let mut a = arena.append(13);

        let ref_a = arena.try_get_mut(&mut a);
        let ref_b = arena.try_get_mut(&mut a);
        let ref_c = arena.try_get(&mut a);

        assert!(ref_a.is_ok());
        assert!(ref_b.is_err());
        assert!(ref_c.is_err());
    }

    #[test]
    fn should_drop() {
        let rcs: Vec<_> = (0..1024).map(|_| Rc::new(0)).collect();
        {
            let arena = Arena::new();

            for rc in rcs.iter() {
                arena.append(rc.clone());
            }

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
        let mut b = arena.append(1);

        assert_eq!(*arena.get(&a), 0);
        *arena.get_mut(&mut b) += 1;

        assert_eq!(*arena.get(&b), 2);
    }

    #[test]
    fn clone() {
        let arena_a = Arena::new();

        let a = arena_a.append(0);
        let mut b = arena_a.append(1);

        // c is cloned from b, since mutably accesing b makes it point to its
        // new memory location.
        let c = b.clone();

        assert_eq!(*arena_a.get(&a), 0);
        assert_eq!(*arena_a.get(&b), 1);

        let arena_b = arena_a.clone();

        // change the value in arena_a
        *arena_a.get_mut(&mut b) += 1;

        // value changed
        assert_eq!(*arena_a.get(&b), 2);

        // old reference `c` is still pointing to the unmodified entry
        assert_eq!(*arena_b.get(&c), 1);
    }

    #[test]
    fn merge() {
        let arena_a = Arena::new();
        let arena_b = Arena::new();

        let a = arena_a.append(0);
        let b = arena_b.append(1);

        // merge the arenas into one, leaving both `a` and `b` accessible
        // through `arena_c`

        let arena_c = Arena::merge(&arena_a, &arena_b);

        assert_eq!(*arena_c.get(&a), 0);
        assert_eq!(*arena_c.get(&b), 1);
    }

    #[test]
    fn debug() {
        let arena = Arena::new();

        let a = arena.append(0);
        let mut b = arena.append(1);

        let _ref_a = arena.try_get(&a);
        let _ref_b = arena.try_get_mut(&mut b);

        // should be able to unsafely borrow in debug output
        let string = format!("{:?}", arena);
        assert_eq!(&string, "{0: [0, 1]}")
    }
}
