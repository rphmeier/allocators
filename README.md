# Scoped Allocator 
[![Build Status](https://travis-ci.org/rphmeier/scoped_allocator.svg)](https://travis-ci.org/rphmeier/scoped_allocator)

This crate provides a scoped linear allocator. This is useful for reusing a block of memory for temporary allocations in a tight loop. Scopes can be nested and values allocated in a scope cannot be moved outside it.

```rust
struct Bomb(u8);
impl Drop for Bomb {
    fn drop(&mut self) {
        println!("Boom! {}", self.0);
    } 
}
// new allocator with a kilobyte of memory.
let alloc = ScopedAllocator::new(1024).unwrap();

alloc.scope(|inner| {
    let mut bombs = Vec::new();
    for i in 0..100 { bombs.push(inner.allocate_val(Bomb(i)).ok().unwrap())}

    // watch the bombs go off!
});

let my_int = alloc.allocate_val(23).ok().unwrap();
println!("My int: {}", *my_int);
```

Disclaimer: this crate leans heavily on unsafe code and nightly features and should not be used in production as it stands.