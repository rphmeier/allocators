# Allocators 
[![Build Status](https://travis-ci.org/rphmeier/allocators.svg)](https://travis-ci.org/rphmeier/allocators) [![Crates.io](https://img.shields.io/crates/v/allocators.svg)](https://crates.io/crates/allocators)

## [Documentation](https://rphmeier.github.io/allocators/allocators/)

This crate provides a different memory allocators, as well as an 
`Allocator` trait for creating other custom allocators. A main goal of allocators is composability. For this reason, it also provides some composable primitives to be used as building blocks for chained allocators. This crate leans heavily on unsafe/unstable code at the moment, and should be considered very experimental. 

# Why?
For Rust to fulfill its description as a systems programming language, users need to have more fine-grained control over the way memory is allocated in their programs. This crate is a proof-of-concept that these mechanisms can be implemented in Rust and provide a safe interface to their users.

# Allocator Traits
## Allocator
This is the core trait for allocators to implement. All a type has to do is implement two unsafe functions: `allocate_raw` and `deallocate_raw`. This will likely require `reallocate_raw` in the future.

## BlockOwner
Allocators that implement this can say definitively whether they own a block.

# Allocator Types
## Scoped Allocator
This is useful for reusing a block of memory for temporary allocations in a tight loop. Scopes can be nested and values allocated in a scope cannot be moved outside it.

```rust
#![feature(placement_in_syntax)]
use allocators::{Allocator, Scoped};
#[derive(Debug)]
struct Bomb(u8);
impl Drop for Bomb {
    fn drop(&mut self) {
        println!("Boom! {}", self.0);
    }
}
// new scoped allocator with a kilobyte of memory.
let alloc = Scoped::new(1024).unwrap();
alloc.scope(|inner| {
    let mut bombs = Vec::new();
    // allocate_val makes the value on the stack first.
    for i in 0..100 { bombs.push(inner.allocate(Bomb(i)).unwrap())}
    // watch the bombs go off!
});
// Allocators also have placement-in syntax.
let my_int = in alloc.make_place().unwrap() { 23 };
println!("My int: {}", *my_int);
```

## Free List Allocator
This allocator maintains a list of free blocks of a given size.
```rust
use allocators::{Allocator, FreeList};

// create a FreeList allocator with 64 blocks of 1024 bytes.
let alloc = FreeList::new(1024, 64).unwrap();
for _ in 0..10 {
    // allocate every block
    let mut v = Vec::new();
    for i in 0u8..64 {
        v.push(alloc.allocate([i; 1024]).unwrap());
    }
    // no more blocks :(.
    assert!(alloc.allocate([0; 1024]).is_err());

    // all the blocks get pushed back onto the freelist at the end here, 
    // memory gets reused efficiently in the next iteration.
}
```

This allocator can yield very good performance for situations like the above, where each block's space is being fully used.

# Composable Primitives
These are very underdeveloped at the moment, and lack a fluent API as well. They are definitely a back-burner feature at the moment, since the idea of composable allocators hasn't really proved its value yet.

## Null Allocator
This will probably get a new name since "Null" has some misleading connotations.

It fails to allocate any request made to it, and panics when deallocated with.

## Fallback Allocator
This composes two `BlockOwners`: a main allocator and a fallback. If the main allocator fails to allocate, it turns to the fallback.

## Proxy Allocator
This wraps any allocator and something which implements the `ProxyLogger` trait, which provides functions to log allocation, deallocation, and reallocation in any arbitrary way. It has practical applications in debug builds for measuring how an allocator is being utilized.