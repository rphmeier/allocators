//! A scoped linear allocator.
//! This is useful for reusing a block of memory for temporary allocations within
//! a tight inner loop. Multiple nested scopes can be used if desired.
//!
//! # Examples
//! ```rust
//!
//! #![feature(placement_in_syntax)]
//! use scoped_allocator::{Allocator, ScopedAllocator};
//! struct Bomb(u8);
//! impl Drop for Bomb {
//!     fn drop(&mut self) {
//!         println!("Boom! {}", self.0);
//!     }
//! }
//! // new scoped allocator with a kilobyte of memory.
//! let alloc = ScopedAllocator::new(1024).unwrap();
//!
//! alloc.scope(|inner| {
//!     let mut bombs = Vec::new();
//!     // allocate_val makes the value on the stack first.
//!     for i in 0..100 { bombs.push(inner.allocate_val(Bomb(i)).ok().unwrap())}
//!     // watch the bombs go off!
//! });
//!
//! // Allocators also have placement-in syntax.
//! let my_int = in alloc.allocate().unwrap() { 23 };
//! println!("My int: {}", *my_int);
//!
//! ```

#![feature(
    alloc,
    coerce_unsized,
    core_intrinsics,
    heap_api,
    placement_new_protocol,
    placement_in_syntax,
    raw,
    unsize
)]

use std::any::Any;
use std::borrow::{Borrow, BorrowMut};
use std::error::Error;
use std::fmt;
use std::marker::Unsize;
use std::mem;
use std::ops::Place as StdPlace;
use std::ops::{CoerceUnsized, Deref, DerefMut, InPlace, Placer};

use alloc::heap;

extern crate alloc;

pub mod scoped;
pub use scoped::ScopedAllocator;

/// A custom memory allocator.
pub unsafe trait Allocator {
    /// Attempts to allocate space for the value supplied to it.
    /// This incurs an expensive memcpy which often dwarfs the
    /// cost of the actual allocation. If the performance of this allocation
    /// is important to you, it is recommended to use the in-place syntax
    /// with the `allocate` function.
    fn allocate_val<T>(&self, val: T) -> Result<Allocated<T, Self>, (AllocatorError, T)>
        where Self: Sized
    {
        use std::ptr;

        let (size, align) = (mem::size_of::<T>(), mem::align_of::<T>());
        match unsafe { self.allocate_raw(size, align) } {
            Ok(ptr) => {
                let item = ptr as *mut T;
                unsafe { ptr::write(item, val) };
                Ok(Allocated {
                    item: item,
                    allocator: self,
                    size: size,
                    align: align,
                })
            }
            Err(e) => Err((e, val)),
        }
    }

    /// Attempts to create a place to allocate into.
    /// 
    /// # Examples
    /// ```rust
    /// #![feature(placement_in_syntax)]
    /// use scoped_allocator::{Allocator, Allocated};
    /// fn alloc_array<A: Allocator>(allocator: &A) -> Allocated<[u8; 1000], A> {
    ///     in allocator.allocate().unwrap() { [0; 1000] }
    /// }
    /// ```
    fn allocate<T>(&self) -> Result<Place<T, Self>, AllocatorError> 
        where Self: Sized
    {
        let (size, align) = (mem::size_of::<T>(), mem::align_of::<T>());
        match unsafe { self.allocate_raw(size, align) } {
            Ok(ptr) => {
                Ok(Place {
                    ptr: ptr as *mut T,
                    allocator: self,
                    finalized: false,
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
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<*mut u8, AllocatorError>;

    /// Deallocate the memory referred to by this pointer.
    ///
    /// # Safety
    /// This pointer must have been allocated by this allocator.
    /// The size and align must be the same as when they were allocated.
    /// Do not deallocate the same pointer twice. Behavior is implementation-defined,
    /// but usually it will not behave as expected.
    unsafe fn deallocate_raw(&self, ptr: *mut u8, size: usize, align: usize);
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
pub const HEAP: &'static HeapAllocator = &HeapAllocator;

unsafe impl Allocator for HeapAllocator {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<*mut u8, AllocatorError> {
        let ptr = if size != 0 {
            heap::allocate(size, align)
        } else {
            heap::EMPTY as *mut u8
        };

        if ptr.is_null() {
            Err(AllocatorError::OutOfMemory)
        } else {
            Ok(ptr)
        }
    }

    unsafe fn deallocate_raw(&self, ptr: *mut u8, size: usize, align: usize) {
        heap::deallocate(ptr, size, align)
    }
}

/// An item allocated by a custom allocator.
pub struct Allocated<'a, T: 'a + ?Sized, A: 'a + Allocator> {
    item: *mut T,
    allocator: &'a A,
    size: usize,
    align: usize,
}

impl<'a, T: ?Sized, A: Allocator> Deref for Allocated<'a, T, A> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { mem::transmute(self.item) }
    }
}

impl<'a, T: ?Sized, A: Allocator> DerefMut for Allocated<'a, T, A> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { mem::transmute(self.item) }
    }
}

// Allocated can store trait objects!
impl<'a, T: ?Sized + Unsize<U>, U: ?Sized, A: Allocator> CoerceUnsized<Allocated<'a, U, A>> for Allocated<'a, T, A> {}

impl<'a, A: Allocator> Allocated<'a, Any, A> {
    /// Attempts to downcast this `Allocated` to a concrete type.
    pub fn downcast<T: Any>(self) -> Result<Allocated<'a, T, A>, Allocated<'a, Any, A>> {
        use std::raw::TraitObject;
        if self.is::<T>() {
            let obj: TraitObject = unsafe { mem::transmute(self.item as *mut Any) };
            let new_allocated = Allocated {
                item: unsafe { mem::transmute(obj.data) },
                allocator: self.allocator,
                size: self.size,
                align: self.align,
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
            self.allocator.deallocate_raw(self.item as *mut u8, self.size, self.align);
        }

    }
}

/// A place for allocating into.
/// This is only used for in-place allocation,
/// e.g. let val = in (alloc.allocate().unwrap())
pub struct Place<'a, T: 'a, A: 'a + Allocator> {
    allocator: &'a A,
    ptr: *mut T,
    finalized: bool
}

impl<'a, T: 'a, A: 'a + Allocator> Placer<T> for Place<'a, T, A> {
    type Place = Self;
    fn make_place(self) -> Self { self }
}

impl<'a, T: 'a, A: 'a + Allocator> InPlace<T> for Place<'a, T, A> {
    type Owner = Allocated<'a, T, A>;
    unsafe fn finalize(mut self) -> Self::Owner {
        self.finalized = true;
        Allocated {
            item: self.ptr,
            allocator: self.allocator,
            size: mem::size_of::<T>(),
            align: mem::align_of::<T>(),
        }
    }
}

impl<'a, T: 'a, A: 'a + Allocator> StdPlace<T> for Place<'a, T, A> {
    fn pointer(&mut self) -> *mut T {
        self.ptr
    }
}

impl<'a, T: 'a, A: 'a + Allocator> Drop for Place<'a, T, A> {
    #[inline]
    fn drop(&mut self) {
        // almost identical to Allocated::Drop, but we only drop if this
        // was never finalized. If it was finalized, an Allocated manages this memory.
        use std::intrinsics::drop_in_place;
        if !self.finalized { unsafe {
            let (size, align) = (mem::size_of::<T>(), mem::align_of::<T>());
            drop_in_place(self.ptr);
            self.allocator.deallocate_raw(self.ptr as *mut u8, size, align);
        } }

    }
}