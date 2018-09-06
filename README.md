# Trove

An arena allocator.

The arena keeps every appended value in a fixed memory location, and only deallocates them all at once.

The arena also allows safe mutable access to the stored elements, through a `RefCell` wrapper.

# Example

```rust
let arena = Arena::new();

let a = arena.append(0);
let b = arena.append(1);

assert_eq!(*arena.get(&a), 0);

*arena.get_mut(&b) += 1;

assert_eq!(*arena.get(&b), 2);

```