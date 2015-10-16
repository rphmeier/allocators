# Allocators 
[![Build Status](https://travis-ci.org/rphmeier/allocators.svg)](https://travis-ci.org/rphmeier/allocators)

## [Documentation](https://rphmeier.github.io/allocators/allocators/)

This crate provides a different memory allocators, as well as an 
`Allocator` trait for creating other custom allocators. A main goal of allocators is composability. For this reason, it also provides some composable primitives to be used as building blocks for chained allocators. This crate leans heavily on unsafe/unstable code at the moment, and should be considered very experimental. 

# Why?
For Rust to fulfill its description as a systems programming language, users need to have more fine-grained control over the way memory is allocated in their programs. This crate is a proof-of-concept that these mechanisms can be implemented in Rust and provide a safe interface to their users.

# Scoped Allocator
This is useful for reusing a block of memory for temporary allocations in a tight loop. Scopes can be nested and values allocated in a scope cannot be moved outside it.

```rust
#![feature(placement_in_syntax)]
use allocators::{Allocator, ScopedAllocator};
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
