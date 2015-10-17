//! Custom memory allocators and utilities for using them.
//!
//! # Examples
//! ```rust
//! #![feature(placement_in_syntax)]
//!
//! use std::io;
//! use allocators::{Allocator, Scoped, BlockOwner, Proxy};
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
//!     let secondary_alloc = Scoped::new_from(&alloc, 1024).unwrap();
//!     let mut val = secondary_alloc.allocate(0i32).unwrap();
//!     *val = 1;
//! }
//!
//! // Let's wrap our allocator in a proxy to log what it's doing.
//! let proxied = Proxy::new(alloc, io::stdout());
//! let logged_allocation = proxied.allocate([0u8; 32]).unwrap();
//! ```

#![feature(
    alloc,
    coerce_unsized,
    core_intrinsics,
    heap_api,
    placement_new_protocol,
    placement_in_syntax,
    raw,
    unsize,
)]

use std::any::Any;
use std::borrow::{Borrow, BorrowMut};
use std::error::Error;
use std::fmt;
use std::marker::{PhantomData, Unsize};
use std::mem;
use std::ops::Place as StdPlace;
use std::ops::{CoerceUnsized, Deref, DerefMut, InPlace, Placer};

use alloc::heap;

extern crate alloc;

pub mod composable;
pub mod scoped;

pub use composable::*;
pub use scoped::Scoped;

/// A custom memory allocator.
pub unsafe trait Allocator {
    /// Attempts to allocate the value supplied to it.
    ///
    /// # Examples
    /// ```rust
    /// use allocators::{Allocator, Allocated};
    /// fn alloc_array<A: Allocator>(allocator: &A) -> Allocated<[u8; 1000], A> {
    ///     allocator.allocate([0; 1000]).ok().unwrap()
    /// }
    /// ```
    #[inline(always)]
    fn allocate<T>(&self, val: T) -> Result<Allocated<T, Self>, (AllocatorError, T)>
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
    /// use allocators::{Allocator, Allocated};
    /// fn alloc_array<A: Allocator>(allocator: &A) -> Allocated<[u8; 1000], A> {
    ///     // if 1000 bytes were enough to smash the stack, this would still work.
    ///     in allocator.make_place().unwrap() { [0; 1000] }
    /// }
    /// ```
    fn make_place<T>(&self) -> Result<Place<T, Self>, AllocatorError> 
        where Self: Sized
    {
        let (size, align) = (mem::size_of::<T>(), mem::align_of::<T>());
        match unsafe { self.allocate_raw(size, align) } {
            Ok(blk) => {
                Ok(Place {
                    allocator: self,
                    block: blk,
                    _marker: PhantomData
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Attempt to allocate a block of memory.
    ///
    /// Returns either a pointer to the block of memory allocated
    /// or an Error. If `size` is equal to 0, the pointer returned must
    /// be equal to `heap::EMPTY`
    ///
    /// # Safety
    /// Never use the pointer outside of the lifetime of the allocator.
    /// It must be deallocated with the same allocator as it was allocated with.
    /// It is undefined behavior to provide a non power-of-two align.
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, AllocatorError>;

    /// Deallocate the memory referred to by this pointer.
    ///
    /// # Safety
    /// This block must have been allocated by this allocator.
    unsafe fn deallocate_raw(&self, blk: Block);
}

/// An allocator that knows which blocks have been issued by it.
pub trait BlockOwner: Allocator {
    /// Whether this allocator owns this allocated value. 
    fn owns<'a, T, A: Allocator>(&self, val: &Allocated<'a, T, A>) -> bool {
        self.owns_block(&val.block)
    }

    /// Whether this allocator owns the block passed to it.
    fn owns_block(&self, blk: &Block) -> bool;

    /// Joins this allocator with a fallback allocator.
    // TODO: Maybe not the right place for this?
    // Right now I've been more focused on shaking out the
    // specifics of allocation than crafting a fluent API.
    fn with_fallback<O: BlockOwner>(self, other: O) -> Fallback<Self, O>
    where Self: Sized {
        Fallback::new(self, other)
    }
}

/// A block of memory created by an allocator.
// TODO: should blocks be tied to lifetimes? seems good for safety!
pub struct Block<'a> {
    ptr: *mut u8,
    size: usize,
    align: usize,
    _marker: PhantomData<&'a [u8]>
}

impl<'a> Block<'a> {
    /// Create a new block from the supplied parts.
    pub fn new(ptr: *mut u8, size: usize, align: usize) -> Self {
        Block {
            ptr: ptr,
            size: size,
            align: align,
            _marker: PhantomData,
        }
    }

    /// Creates an empty block.
    pub fn empty() -> Self {
        Block {
            ptr: heap::EMPTY as *mut u8,
            size: 0,
            align: 0,
            _marker: PhantomData,
        }
    }

    /// Get the pointer from this block.
    pub fn ptr(&self) -> *mut u8 { self.ptr }
    /// Get the size of this block.
    pub fn size(&self) -> usize { self.size }
    /// Get the align of this block.
    pub fn align(&self) -> usize { self.align }
    /// Whether this block is empty.
    pub fn is_empty(&self) -> bool {
        self.ptr as *mut () == heap::EMPTY || self.size == 0
    }
}

impl<'a> Clone for Block<'a> {
    fn clone(&self) -> Block<'a> {
        Block {
            ptr: self.ptr(),
            size: self.size(),
            align: self.align(),
            _marker: self._marker
        }
    }
}

/// Errors that can occur while creating an allocator
/// or allocating from it.
#[derive(Debug, Eq, PartialEq)]
pub enum AllocatorError {
    /// The allocator failed to allocate the amount of memory requested of it.
    OutOfMemory,
    /// An allocator-specific error message.
    AllocatorSpecific(String),
}

impl fmt::Display for AllocatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(self.description())
    }
}

impl Error for AllocatorError {
    fn description(&self) -> &str {
        use AllocatorError::*;

        match *self {
            OutOfMemory => {
                "Allocator out of memory."
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
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, AllocatorError> {
        if size == 0 { return Ok(Block::empty()) }

        let ptr = heap::allocate(size, align);
        if ptr.is_null() {
            Err(AllocatorError::OutOfMemory)
        } else {
            Ok(Block::new(ptr, size, align))
        }
    }

    unsafe fn deallocate_raw(&self, blk: Block) {
        if !blk.is_empty() { 
            heap::deallocate(blk.ptr(), blk.size(), blk.align())
        }
    }
}

/// An item allocated by a custom allocator.
pub struct Allocated<'a, T: 'a + ?Sized, A: 'a + Allocator> {
    item: *mut T,
    allocator: &'a A,
    block: Block<'a>,
}

impl<'a, T: ?Sized, A: Allocator> Deref for Allocated<'a, T, A> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.item }
    }
}

impl<'a, T: ?Sized, A: Allocator> DerefMut for Allocated<'a, T, A> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.item }
    }
}

// Allocated can store trait objects!
impl<'a, T: ?Sized + Unsize<U>, U: ?Sized, A: Allocator> CoerceUnsized<Allocated<'a, U, A>> for Allocated<'a, T, A> {}

impl<'a, A: Allocator> Allocated<'a, Any, A> {
    /// Attempts to downcast this `Allocated` to a concrete type.
    pub fn downcast<T: Any>(self) -> Result<Allocated<'a, T, A>, Allocated<'a, Any, A>> {
        use std::raw::TraitObject;
        if self.is::<T>() {
            let obj: TraitObject = unsafe { mem::transmute::<*mut Any, TraitObject>(self.item) };
            let new_allocated = Allocated {
                item: obj.data as *mut T,
                allocator: self.allocator,
                block: self.block.clone(),
            };
            mem::forget(self);
            Ok(new_allocated)
        } else {
            Err(self)
        }
    }
}

impl<'a, T: ?Sized, A: Allocator> Borrow<T> for Allocated<'a, T, A> {
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<'a, T: ?Sized, A: Allocator> BorrowMut<T> for Allocated<'a, T, A> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<'a, T: ?Sized, A: Allocator> Drop for Allocated<'a, T, A> {
    #[inline]
    fn drop(&mut self) {
        use std::intrinsics::drop_in_place;
        unsafe {
            drop_in_place(self.item);
;
            self.allocator.deallocate_raw(self.block.clone());
        }

    }
}

/// A place for allocating into.
/// This is only used for in-place allocation,
/// e.g. `let val = in (alloc.make_place().unwrap()) { EXPR }`
pub struct Place<'a, T: 'a, A: 'a + Allocator> {
    allocator: &'a A,
    block: Block<'a>,
    _marker: PhantomData<T>,
}

impl<'a, T: 'a, A: 'a + Allocator> Placer<T> for Place<'a, T, A> {
    type Place = Self;
    fn make_place(self) -> Self { self }
}

impl<'a, T: 'a, A: 'a + Allocator> InPlace<T> for Place<'a, T, A> {
    type Owner = Allocated<'a, T, A>;
    unsafe fn finalize(self) -> Self::Owner {
        let allocated = Allocated {
            item: self.block.ptr() as *mut T,
            allocator: self.allocator,
            block: self.block.clone()
        };

        mem::forget(self);
        allocated
    }
}

impl<'a, T: 'a, A: 'a + Allocator> StdPlace<T> for Place<'a, T, A> {
    fn pointer(&mut self) -> *mut T {
        self.block.ptr() as *mut T
    }
}

impl<'a, T: 'a, A: 'a + Allocator> Drop for Place<'a, T, A> {
    #[inline]
    fn drop(&mut self) {
        // almost identical to Allocated::Drop, but we don't drop
        // the value in place. This is because if the finalize
        // method was never called, that means the expression
        // to create the value failed, and the memory at the
        // pointer is still uninitialized.
        unsafe {
            self.allocator.deallocate_raw(self.block.clone());
        }

    }
}

// aligns a pointer forward to the next value aligned with `align`.
#[inline(always)]
fn align_forward(ptr: *mut u8, align: usize) -> *mut u8 {
    ((ptr as usize + align - 1) & !(align - 1)) as *mut u8
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

        let my_foo: Allocated<Any, _> = HEAP.allocate(Bomb).unwrap();
        let _: Allocated<Bomb, _> = my_foo.downcast().ok().unwrap();
    }
}