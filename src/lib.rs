#![deny(missing_docs)]
#![allow(unknown_lints)]
#![allow(mut_from_ref)]

//! When a value is put into the Arena, it will stay there for the whole
//! lifetime of the arena, and never move.
extern crate vec_map;

use std::cell::{Ref, RefCell, RefMut, UnsafeCell};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::{fmt, mem};

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
    len: UnsafeCell<usize>,
}

impl<T> Default for ArenaInner<T> {
    fn default() -> Self {
        ArenaInner {
            rows: UnsafeCell::new(Default::default()),
            len: UnsafeCell::new(0),
        }
    }
}

/// An arena that can hold values of type `T`.
pub struct Arena<T> {
    id: UnsafeCell<usize>,
    arenas: UnsafeCell<VecMap<Rc<ArenaInner<T>>>>,
}

impl<T> Clone for Arena<T> {
    fn clone(&self) -> Self {
        let new_id_a = new_id();
        let new_id_b = new_id();
        let inner_a = ArenaInner::default();
        let inner_b = ArenaInner::default();
        unsafe {
            *self.id.get() = new_id_a;
            let arenas = &mut *self.arenas.get();
            arenas.insert(new_id_a, Rc::new(inner_a));
            arenas.insert(new_id_b, Rc::new(inner_b));
            Arena {
                id: UnsafeCell::new(new_id_b),
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
            id: UnsafeCell::new(id),
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
                id: UnsafeCell::new(new_id()),
                arenas: UnsafeCell::new(new_arenas),
            }
        }
    }

    /// Get a reference into the arena.
    ///
    /// Panics on out-of bound access.
    pub fn get(&self, arena_idx: &ArenaIdx) -> ArenaRef<T> {
        let arenas = unsafe { &mut *self.arenas.get() };
        arenas
            .get(arena_idx.arena)
            .expect("Invalid arena_idx")
            .get(arena_idx.offset)
    }

    /// Get a mutable reference into the arena.
    /// Panics if aliased, through a `RefCell` wrapper
    pub fn get_mut(&self, arena_idx: &mut ArenaIdx) -> ArenaRefMut<T> {
        let arenas = unsafe { &mut *self.arenas.get() };
        let id = unsafe { *self.id.get() };
        if arena_idx.arena == id {
            arenas
                .get(arena_idx.arena)
                .expect("Invalid arena_idx")
                .get_mut(arena_idx.offset)
        } else {
            let t: T = (*self.get(arena_idx)).clone();
            *arena_idx = self.append(t);
            self.get_mut(arena_idx)
        }
    }

    /// Puts a value into the arena, returning an index
    pub fn append(&self, t: T) -> ArenaIdx {
        let arenas = unsafe { &mut *self.arenas.get() };
        let id = unsafe { *self.id.get() };
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
    fn get(&self, offset: usize) -> ArenaRef<T> {
        if offset >= unsafe { *self.len.get() } {
            panic!("Index out of bounds")
        }
        let (row, col) = Self::index(offset);
        unsafe {
            let rows = self.rows.get();
            ArenaRef((*rows)[row][col].borrow())
        }
    }

    pub fn get_mut(&self, offset: usize) -> ArenaRefMut<T> {
        if offset >= unsafe { *self.len.get() } {
            panic!("Index out of bounds")
        }
        let (row, col) = Self::index(offset);
        unsafe {
            let rows = &mut *self.rows.get();
            ArenaRefMut(rows[row][col].borrow_mut())
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
        let i = unsafe { *self.len.get() };
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
        unsafe {
            *self.len.get() += 1;
        }
        ArenaIdx {
            offset: i,
            arena: id,
            _marker: PhantomData,
        }
    }
}

impl<T> fmt::Debug for ArenaInner<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            let len = *self.len.get();
            write!(f, "[")?;
            if len > 0 {
                write!(f, "{:?}", self.get(0))?;
                for i in 1..len {
                    write!(f, ", {:?}", self.get(i))?
                }
            }
            write!(f, "]")
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
        let c = b.clone();

        assert_eq!(*arena_a.get(&a), 0);
        assert_eq!(*arena_a.get(&b), 1);

        let arena_b = arena_a.clone();

        *arena_a.get_mut(&mut b) += 1;

        assert_eq!(*arena_a.get(&b), 2);
        assert_eq!(*arena_b.get(&c), 1);
    }

    #[test]
    fn merge() {
        let arena_a = Arena::new();
        let arena_b = Arena::new();

        let a = arena_a.append(0);
        let b = arena_b.append(0);

        let arena_c = Arena::merge(&arena_a, &arena_b);

        assert_eq!(*arena_c.get(&a), 0);
        assert_eq!(*arena_c.get(&b), 0);
    }
}
