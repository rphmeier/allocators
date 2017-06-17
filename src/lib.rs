//! Custom memory allocators and utilities for using them.
//!
//! # Examples
//! ```rust
//! #![feature(placement_in_syntax)]
//!
//! use std::io;
//! use allocators::{Allocator, Scoped, BlockOwner, FreeList, Proxy};
//!
//! #[derive(Debug)]
//! struct Bomb(u8);
//!
//! impl Drop for Bomb {
//!     fn drop(&mut self) {
//!         println!("Boom! {}", self.0);
//!     }
//! }
//! // new scoped allocator with 4 kilobytes of memory.
//! let alloc = Scoped::new(4 * 1024).unwrap();
//!
//! alloc.scope(|inner| {
//!     let mut bombs = Vec::new();
//!     // allocate makes the value on the stack first.
//!     for i in 0..100 { bombs.push(inner.allocate(Bomb(i)).unwrap())}
//!     // there's also in-place allocation!
//!     let bomb_101 = in inner.make_place().unwrap() { Bomb(101) };
//!     // watch the bombs go off!
//! });
//!
//!
//! // You can make allocators backed by other allocators.
//! {
//!     let secondary_alloc = FreeList::new_from(&alloc, 128, 8).unwrap();
//!     let mut val = secondary_alloc.allocate(0i32).unwrap();
//!     *val = 1;
//! }
//!
//! ```

#![feature(
    alloc,
    coerce_unsized,
    heap_api,
    placement_new_protocol,
    placement_in_syntax,
    raw,
    unique,
    unsize,
)]

use std::error::Error as StdError;
use std::fmt;
use std::marker::PhantomData;
use std::ptr::Unique;

use alloc::heap;

extern crate alloc;

mod boxed;
pub mod composable;
pub mod freelist;
pub mod scoped;

pub use boxed::{AllocBox, Place};
pub use composable::*;
pub use freelist::FreeList;
pub use scoped::Scoped;

/// A custom memory allocator.
pub unsafe trait Allocator {
    /// Attempts to allocate the value supplied to it.
    ///
    /// # Examples
    /// ```rust
    /// use allocators::{Allocator, AllocBox};
    /// fn alloc_array<A: Allocator>(allocator: &A) -> AllocBox<[u8; 1000], A> {
    ///     allocator.allocate([0; 1000]).ok().unwrap()
    /// }
    /// ```
    #[inline]
    fn allocate<T>(&self, val: T) -> Result<AllocBox<T, Self>, (Error, T)>
    where Self: Sized
    {
        match self.make_place() {
            Ok(place) => {
                Ok(in place { val })
            }
            Err(err) => {
                Err((err, val))
            }
        }
    }

    /// Attempts to create a place to allocate into.
    /// For the general purpose, calling `allocate` on the allocator is enough.
    /// However, when you know the value you are allocating is too large
    /// to be constructed on the stack, you should use in-place allocation.
    /// 
    /// # Examples
    /// ```rust
    /// #![feature(placement_in_syntax)]
    /// use allocators::{Allocator, AllocBox};
    /// fn alloc_array<A: Allocator>(allocator: &A) -> AllocBox<[u8; 1000], A> {
    ///     // if 1000 bytes were enough to smash the stack, this would still work.
    ///     in allocator.make_place().unwrap() { [0; 1000] }
    /// }
    /// ```
    fn make_place<T>(&self) -> Result<Place<T, Self>, Error>
    where Self: Sized
    {
        boxed::make_place(self)
    }
    
    /// Attempt to allocate a block of memory.
    ///
    /// Returns either a block of memory allocated
    /// or an Error. If `size` is equal to 0, the block returned must
    /// be created by `Block::empty()`
    ///
    /// # Safety
    /// Never use the block's pointer outside of the lifetime of the allocator.
    /// It must be deallocated with the same allocator as it was allocated with.
    /// It is undefined behavior to provide a non power-of-two align.
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, Error>;

    /// Reallocate a block of memory.
    ///
    /// This either returns a new, possibly moved block with the requested size,
    /// or the old block back.
    /// The new block will have the same alignment as the old.
    ///
    /// # Safety
    /// If given an empty block, it must return it back instead of allocating the new size,
    /// since the alignment is unknown.
    ///
    /// If the requested size is 0, it must deallocate the old block and return an empty one.
    unsafe fn reallocate_raw<'a>(&'a self, block: Block<'a>, new_size: usize) -> Result<Block<'a>, (Error, Block<'a>)>;

    /// Deallocate the memory referred to by this block.
    ///
    /// # Safety
    /// This block must have been allocated by this allocator.
    unsafe fn deallocate_raw(&self, block: Block);
}

/// An allocator that knows which blocks have been issued by it.
pub trait BlockOwner: Allocator {
    /// Whether this allocator owns this allocated value. 
    fn owns<'a, T, A: Allocator>(&self, val: &AllocBox<'a, T, A>) -> bool {
        self.owns_block(& unsafe { val.as_block() })
    }

    /// Whether this allocator owns the block passed to it.
    fn owns_block(&self, block: &Block) -> bool;

    /// Joins this allocator with a fallback allocator.
    // TODO: Maybe not the right place for this?
    // Right now I've been more focused on shaking out the
    // specifics of allocation than crafting a fluent API.
    fn with_fallback<O: BlockOwner>(self, other: O) -> Fallback<Self, O>
        where Self: Sized
    {
        Fallback::new(self, other)
    }
}

/// A block of memory created by an allocator.
pub struct Block<'a> {
    ptr: Unique<u8>,
    size: usize,
    align: usize,
    _marker: PhantomData<&'a [u8]>,
}

impl<'a> Block<'a> {
    /// Create a new block from the supplied parts.
    /// The pointer cannot be null.
    ///
    /// # Panics
    /// Panics if the pointer passed is null.
    pub fn new(ptr: *mut u8, size: usize, align: usize) -> Self {
        assert!(!ptr.is_null());
        Block {
            ptr: unsafe { Unique::new(ptr) },
            size: size,
            align: align,
            _marker: PhantomData,
        }
    }

    /// Creates an empty block.
    pub fn empty() -> Self {
        Block {
            ptr: Unique::empty(),
            size: 0,
            align: 0,
            _marker: PhantomData,
        }
    }

    /// Get the pointer from this block.
    pub fn ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
    /// Get the size of this block.
    pub fn size(&self) -> usize {
        self.size
    }
    /// Get the align of this block.
    pub fn align(&self) -> usize {
        self.align
    }
    /// Whether this block is empty.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

/// Errors that can occur while creating an allocator
/// or allocating from it.
#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    /// The allocator failed to allocate the amount of memory requested of it.
    OutOfMemory,
    /// The allocator does not support the requested alignment.
    UnsupportedAlignment,
    /// An allocator-specific error message.
    AllocatorSpecific(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(self.description())
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        use Error::*;

        match *self {
            OutOfMemory => {
                "Allocator out of memory."
            }
            UnsupportedAlignment => {
                "Attempted to allocate with unsupported alignment."
            }
            AllocatorSpecific(ref reason) => {
                reason
            }
        }
    }
}

/// Allocator stub that just forwards to heap allocation.
/// It is recommended to use the `HEAP` constant instead
/// of creating a new instance of this, to benefit from
/// the static lifetime that it provides.
#[derive(Debug)]
pub struct HeapAllocator;

// A constant for allocators to use the heap as a root.
// Values allocated with this are effectively `Box`es.
pub const HEAP: &'static HeapAllocator = &HeapAllocator;

unsafe impl Allocator for HeapAllocator {
    #[inline]
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, Error> {
        if size != 0 {
            let ptr = heap::allocate(size, align);
            if !ptr.is_null() {
                Ok(Block::new(ptr, size, align))
            } else {
                Err(Error::OutOfMemory)
            }
        } else {
            Ok(Block::empty())
        }
    }

    #[inline]
    unsafe fn reallocate_raw<'a>(&'a self, block: Block<'a>, new_size: usize) -> Result<Block<'a>, (Error, Block<'a>)> {
        if new_size == 0 {
            self.deallocate_raw(block);
            Ok(Block::empty())
        } else if block.is_empty() {
            Err((Error::UnsupportedAlignment, block))
        } else {
            let new_ptr = heap::reallocate(block.ptr(), block.size(), new_size, block.align());

            if new_ptr.is_null() {
                Err((Error::OutOfMemory, block))
            } else {
                Ok(Block::new(new_ptr, new_size, block.align()))
            }
        }
    }

    #[inline]
    unsafe fn deallocate_raw(&self, block: Block) {
        if !block.is_empty() {
            heap::deallocate(block.ptr(), block.size(), block.align())
        }
    }
}

// aligns a pointer forward to the next value aligned with `align`.
#[inline]
fn align_forward(ptr: *mut u8, align: usize) -> *mut u8 {
    ((ptr as usize + align - 1) & !(align - 1)) as *mut u8
}

// implementations for trait object types.

unsafe impl<'a, A: ?Sized + Allocator + 'a> Allocator for Box<A> {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, Error> {
        (**self).allocate_raw(size, align)
    }

    unsafe fn reallocate_raw<'b>(&'b self, block: Block<'b>, new_size: usize) -> Result<Block<'b>, (Error, Block<'b>)> {
        (**self).reallocate_raw(block, new_size)
    }

    unsafe fn deallocate_raw(&self, block: Block) {
        (**self).deallocate_raw(block)
    }
}

unsafe impl<'a, 'b: 'a, A: ?Sized + Allocator + 'b> Allocator for &'a A {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, Error> {
        (**self).allocate_raw(size, align)
    }

    unsafe fn reallocate_raw<'c>(&'c self, block: Block<'c>, new_size: usize) -> Result<Block<'c>, (Error, Block<'c>)> {
        (**self).reallocate_raw(block, new_size)
    }

    unsafe fn deallocate_raw(&self, block: Block) {
        (**self).deallocate_raw(block)
    }
}

unsafe impl<'a, 'b: 'a, A: ?Sized + Allocator + 'b> Allocator for &'a mut A {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, Error> {
        (**self).allocate_raw(size, align)
    }

    unsafe fn reallocate_raw<'c>(&'c self, block: Block<'c>, new_size: usize) -> Result<Block<'c>, (Error, Block<'c>)> {
        (**self).reallocate_raw(block, new_size)
    }

    unsafe fn deallocate_raw(&self, block: Block) {
        (**self).deallocate_raw(block)
    }
}

#[cfg(test)]
mod tests {

    use std::any::Any;

    use super::*;

    #[test]
    fn heap_lifetime() {
        let my_int;
        {
            my_int = HEAP.allocate(0i32).unwrap();
        }

        assert_eq!(*my_int, 0);
    }
    #[test]
    fn heap_in_place() {
        let big = in HEAP.make_place().unwrap() { [0u8; 8_000_000] };
        assert_eq!(big.len(), 8_000_000);
    }

    #[test]
    fn unsizing() {
        #[derive(Debug)]
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) {
                println!("Boom")
            }
        }

        let my_foo: AllocBox<Any, _> = HEAP.allocate(Bomb).unwrap();
        let _: AllocBox<Bomb, _> = my_foo.downcast().ok().unwrap();
    }

    #[test]
    fn take_out() {
        let _: [u8; 1024] = HEAP.allocate([0; 1024]).ok().unwrap().take();
    }

    #[test]
    fn boxed_allocator() {
        #[derive(Debug)]
        struct Increment<'a>(&'a mut i32);
        impl<'a> Drop for Increment<'a> {
            fn drop(&mut self) {
                *self.0 += 1;
            }
        }

        let mut i = 0;
        let alloc: Box<Allocator> = Box::new(HEAP);
        {
            let _ = alloc.allocate(Increment(&mut i)).unwrap();
        }
        assert_eq!(i, 1);
    }
}
