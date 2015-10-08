# Scoped Allocator 
[![Build Status](https://travis-ci.org/rphmeier/scoped_allocator.svg)](https://travis-ci.org/rphmeier/scoped_allocator)

This crate provides a scoped linear allocator. This is useful for reusing a block of memory for temporary allocations in a tight loop. Scopes can be nested and values allocated in a scope cannot be moved outside it.

```rust
#![feature(placement_in_syntax)]
use scoped_allocator::{Allocator, ScopedAllocator};
#[derive(Debug)]
struct Bomb(u8);
impl Drop for Bomb {
    fn drop(&mut self) {
        println!("Boom! {}", self.0);
    }
}
// new scoped allocator with a kilobyte of memory.
let alloc = ScopedAllocator::new(1024).unwrap();
alloc.scope(|inner| {
    let mut bombs = Vec::new();
    // allocate_val makes the value on the stack first.
    for i in 0..100 { bombs.push(inner.allocate_val(Bomb(i)).unwrap())}
    // watch the bombs go off!
});
// Allocators also have placement-in syntax.
let my_int = in alloc.allocate().unwrap() { 23 };
println!("My int: {}", *my_int);
```

Disclaimer: this crate leans heavily on unsafe code and nightly features and should not be used in production as it stands.