# Scoped Allocator
=====

This crate provides a scoped linear allocator. This is useful for reusing a block of memory for temporary allocations in a tight loop. Scopes can be nested and values allocated in a scope cannot be moved outside it.

```rust
struct Bomb(u8);
impl Drop for Bomb {
    fn drop(&mut self) {
        println!("Boom! {}", self.0);
    } 
}
// new allocator with a kilobyte of memory.
let alloc = ScopedAllocator::new(1024);

alloc.scope(|| {
    let mut bombs = Vec::new();
    for i in 0..100 { bombs.push(alloc.allocate(Bomb(i)).ok().unwrap())}

    // watch the bombs go off!
});

let my_int = alloc.allocate(23);
println!("My int: {}", *my_int);
```