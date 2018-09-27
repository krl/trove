# Trove

[Documentation](https://docs.rs/trove/0.4.2/trove/)

A cloneable and mergeable, thread local arena allocator.

Trove is designed as a flexible allocator and dynamic borrow checker for immutable datastructures.

The arena keeps every appended value in a fixed memory location, and only deallocates them all at once. This is achieved by using an array of multiple backing vectors, arranged in increasing length, like so:

# Cloning and Merging

Arenas can be cloned and merged, this is accopmlished by the top-level `Arena` type being a `VecMap` mapping increasing ids to sub-arenas. When an arena is cloned, _two_ new sub arenas are created, and given two new ids.

If a mutable borrow is requested through a reference to the old arena, from either of the cloned arenas, it is cloned and put in the new sub-arena, so that the old reference stays valid.

```rust
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
```

# Memory Layout

Each sub-arena has its memory layout arranged like this:

```
[0, 1]
[2, 3, 4, 5]
[6, 7, 8, 9, 10, 11, 12, 13]
```

This implementation uses a first row vector size of 32, and the doubling vectors arranged in 32 rows.

This makes sure no re-allocation and moving of entries is possible, and allows us to safely create a `RefMut` from an immutable borrow of the arena.
