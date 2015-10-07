//! A scoped linear allocator.
//! This is useful for reusing a block of memory for temporary allocations within
//! a tight inner loop. Multiple nested scopes can be used if desired.
//!
//! # Examples
//! ```rust
//! use scoped_allocator::{Allocator, ScopedAllocator};
//! struct Bomb(u8);
//! impl Drop for Bomb {
//!     fn drop(&mut self) {
//!         println!("Boom! {}", self.0);
//!     } 
//! }
//! // new allocator with a kilobyte of memory.
//! let alloc = ScopedAllocator::new(1024).unwrap();
//!
//! alloc.scope(|inner| {
//!     let mut bombs = Vec::new();
//!     for i in 0..100 { bombs.push(inner.allocate(Bomb(i)).ok().unwrap())}
//!     // watch the bombs go off!
//! });
//! 
//! let my_int = alloc.allocate(23).ok().unwrap();
//! println!("My int: {}", *my_int);
//! ```

#![feature(
    alloc, 
    coerce_unsized,
    core_intrinsics,
    heap_api,
    raw,
    unsize
)]

use std::any::Any;
use std::borrow::{Borrow, BorrowMut};
use std::cell::Cell;
use std::error::Error;
use std::fmt;
use std::intrinsics::drop_in_place;
use std::marker::Unsize;
use std::mem;
use std::ops::{CoerceUnsized, Deref, DerefMut};
use std::raw::TraitObject;
use std::ptr;

use alloc::heap;

extern crate alloc;

/// A custom memory allocator.
pub unsafe trait Allocator {
    /// Attempts to allocate space for the value supplied to it.
    /// At the moment, this incurs an expensive memcpy when copying `val`
    /// to the allocated space.
    fn allocate<'a, T>(&'a self, val: T) -> Result<Allocated<'a, T, Self>, (AllocatorError, T)> where Self: Sized {
        let (size, align) = (mem::size_of::<T>(), mem::align_of::<T>());
        match unsafe { self.allocate_raw(size, align) } {
            Ok(ptr) => {
                let item = ptr as *mut T;
                unsafe { ptr::write(item, val) };
                Ok(Allocated {
                    item: item,
                    allocator: self,
                    size: size,
                    align: align
                })
            }
            Err(e) => Err((e, val))
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
#[derive(Debug)]
pub struct HeapAllocator;

// A constant so allocators can use the heap as a root.
const HEAP: &'static HeapAllocator = &HeapAllocator;

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

    fn deref<'b>(&'b self) -> &'b T {
        unsafe { mem::transmute(self.item) }
    }
}

impl<'a, T: ?Sized, A: Allocator> DerefMut for Allocated<'a, T, A> {
    fn deref_mut<'b>(&'b mut self) -> &'b mut T {
        unsafe { mem::transmute(self.item) }
    }
}

// Allocated can store trait objects!
impl<'a, T: ?Sized + Unsize<U>, U: ?Sized, A: Allocator> CoerceUnsized<Allocated<'a, U, A>> for Allocated<'a, T, A> {}

impl<'a, A: Allocator> Allocated<'a, Any, A> {
    /// Attempts to downcast this `Allocated` to a concrete type.
    pub fn downcast<T: Any>(self) -> Result<Allocated<'a, T, A>, Allocated<'a, Any, A>> {
        if self.is::<T>() {
            let obj: TraitObject = unsafe { mem::transmute(self.item as *mut Any) };
            let new_allocated = Allocated { 
                item: unsafe { mem::transmute(obj.data) },
                allocator: self.allocator,
                size: self.size,
                align: self.align
            };
            mem::forget(self);
            Ok(new_allocated)
        } else {
            Err(self)
        }
    }
}

impl<'a, T: ?Sized, A: Allocator> Borrow<T> for Allocated<'a, T, A> {
    fn borrow(&self) -> &T { &**self }
}

impl<'a, T: ?Sized, A: Allocator> BorrowMut<T> for Allocated<'a, T, A> {
    fn borrow_mut(&mut self) -> &mut T { &mut **self }
}

impl<'a, T: ?Sized, A: Allocator> Drop for Allocated<'a, T, A> {
    #[inline]
    fn drop(&mut self) {
        unsafe { 
            drop_in_place(self.item);
            self.allocator.deallocate_raw(self.item as *mut u8, self.size, self.align);
        }

    }
}
/// A scoped linear allocator
pub struct ScopedAllocator<'parent, A: 'parent + Allocator> {
    allocator: &'parent A,
    current: Cell<*mut u8>,
    end: *mut u8,
    root: bool,
    start: *mut u8,
}

impl ScopedAllocator<'static, HeapAllocator> {
    /// Creates a new `ScopedAllocator` backed by `size` bytes from the heap.
    pub fn new(size: usize) -> Result<Self, AllocatorError> {
        ScopedAllocator::new_from(HEAP, size)
    }
}

impl<'parent, A: Allocator> ScopedAllocator<'parent, A> {
    /// Creates a new `ScopedAllocator` backed by `size` bytes from the allocator supplied.
    pub fn new_from(alloc: &'parent A, size: usize) -> Result<Self, AllocatorError> {
        // Create a memory buffer with the desired size and maximal align from the parent.
        match unsafe { alloc.allocate_raw(size, mem::align_of::<usize>()) } {
            Ok(start) => 
                Ok(ScopedAllocator {
                    allocator: alloc,
                    current: Cell::new(start),
                    end: unsafe { start.offset(size as isize) },
                    root: true,
                    start: start,
                }),
            Err(err) => Err(err)
        }
    }

    /// Calls the supplied function with a new scope of the allocator.
    ///
    /// Returns the result of the closure or an error if this allocator
    /// has already been scoped.
    pub fn scope<F, U>(&self, f: F) -> Result<U, ()> 
    where F: FnMut(&Self) -> U {
        if self.is_scoped() {
            return Err(())
        }

        let mut f = f;
        let old = self.current.get();
        let alloc = ScopedAllocator {
            allocator: self.allocator,
            current: self.current.clone(),
            end: self.end,
            root: false,
            start: self.start,
        };
        
        // set the current pointer to null as a flag to indicate
        // that this allocator is being scoped.
        self.current.set(ptr::null_mut());
        let u = f(&alloc);
        self.current.set(old);
        
        mem::forget(alloc);
        Ok(u)
    }

    // Whether this allocator is currently scoped.
    pub fn is_scoped(&self) -> bool {
        self.current.get().is_null()
    }
}

unsafe impl<'a, A: Allocator> Allocator for ScopedAllocator<'a, A> {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<*mut u8, AllocatorError> {
        if self.is_scoped() { 
            return Err(
                AllocatorError::AllocatorSpecific(
                    "Called allocate on already scoped allocator.".into()
                )
            )
        }

        let current_ptr = self.current.get();
        let aligned_ptr = ((current_ptr as usize + align - 1) & !(align - 1)) as *mut u8;
        let end_ptr = aligned_ptr.offset(size as isize);

        if end_ptr > self.end {
            Err(AllocatorError::OutOfMemory)
        } else {
            self.current.set(end_ptr);
            Ok(aligned_ptr)
        }
    }

    #[allow(unused_variables)]
    unsafe fn deallocate_raw(&self, ptr: *mut u8, size: usize, align: usize) {
        // no op for this. The memory gets reused when the scope is cleared.
    }
}

impl<'a, A: Allocator> Drop for ScopedAllocator<'a, A> {
    /// Drops the `ScopedAllocator`
    fn drop(&mut self) {
        let size = self.end as usize - self.start as usize;
        // only free if this allocator is the root to make sure
        // that memory is freed after destructors for allocated objects
        // are called in case of unwind
        if self.root && size > 0 { 
            unsafe { self.allocator.deallocate_raw(self.start, size, mem::align_of::<usize>()) }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    
    use super::*;

    #[test]
    #[should_panic]
    fn use_outer() {
        let alloc = ScopedAllocator::new(4).unwrap();
        let mut outer_val = alloc.allocate(0i32).ok().unwrap();
        alloc.scope(|_inner| {
            // using outer allocator is dangerous and should fail.
            outer_val = alloc.allocate(1i32).ok().unwrap();
        }).unwrap();
    }

    #[test]
    fn unsizing() {
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) { println!("Boom") }
        }

        let alloc = ScopedAllocator::new(4).unwrap();
        let my_foo: Allocated<Any, _> = alloc.allocate(Bomb).ok().unwrap();
        let _: Allocated<Bomb, _> = my_foo.downcast().ok().unwrap();
    }

    #[test]
    fn scope_scope() {
        let alloc = ScopedAllocator::new(64).unwrap();
        let _ = alloc.allocate(0).ok().unwrap();
        alloc.scope(|inner| {
            let _ = inner.allocate(32);
            inner.scope(|bottom| {
                let _ = bottom.allocate(23);
            }).unwrap();
        }).unwrap();
    }

    #[test]
    fn out_of_memory() {
        // allocate more memory than the allocator has.
        let alloc = ScopedAllocator::new(0).unwrap();
        let (err, _) = alloc.allocate(1i32).err().unwrap();
        assert_eq!(err, AllocatorError::OutOfMemory);
    }
}