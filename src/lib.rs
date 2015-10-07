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
//! let alloc = ScopedAllocator::new(1024);
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
    ptr_as_ref,
    raw,
    unsize
)]

use std::any::Any;
use std::borrow::{Borrow, BorrowMut};
use std::cell::Cell;
use std::intrinsics::drop_in_place;
use std::marker::Unsize;
use std::mem;
use std::ops::{CoerceUnsized, Deref, DerefMut};
use std::raw::TraitObject;
use std::ptr;

use alloc::heap;

extern crate alloc;

/// A custom memory allocator.
pub trait Allocator {
    /// Attempts to allocate space for the T supplied to it.
    fn allocate<'a, T>(&'a self, val: T) -> Result<Allocated<'a, T, Self>, T> where Self: Sized {
        let (size, align) = (mem::size_of::<T>(), mem::align_of::<T>());
        match unsafe { self.allocate_raw(size, align) } {
            Ok(ptr) => {
                let item = ptr as *mut T;
                unsafe { ptr::write(item, val) };

                Ok(Allocated {
                    item: unsafe { item.as_mut().expect("allocate returned null ptr") },
                    allocator: self,
                    size: size,
                    align: align
                })
            }
            Err(_) => Err(val)
        }
    }

    /// Attempt to allocate a block of memory.
    ///
    /// # Safety
    /// Never use the pointer outside of the lifetime of the allocator.
    /// It must be deallocated with the same allocator as it was allocated with.
    /// It is undefined behavior to provide a non power-of-two align.
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<*mut u8, ()>;

    /// Deallocate the memory referred to by this pointer.
    ///
    /// # Safety
    /// This pointer must have been allocated by this allocator.
    /// The size and align must be the same as when they were allocated.
    /// Do not deallocate the same pointer twice. Behavior is implementation-defined,
    /// but usually not expected.
    unsafe fn deallocate_raw(&self, ptr: *mut u8, size: usize, align: usize);
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
pub struct ScopedAllocator {
    current: Cell<*mut u8>,
    end: *mut u8,
    start: *mut u8,
}

impl ScopedAllocator {
    /// Creates a new `ScopedAllocator` backed by `size` bytes.
    pub fn new(size: usize) -> Self {
        // Create a memory buffer with the desired size and maximal align.
        let start = if size != 0 {
            unsafe { heap::allocate(size, mem::align_of::<usize>()) }
        } else {
            heap::EMPTY as *mut u8
        };

        if start.is_null() {
            // do result-based error management instead?
            panic!("Out of memory!");
        }

        ScopedAllocator {
            current: Cell::new(start),
            end: unsafe { start.offset(size as isize) },
            start: start,
        }
    }

    /// Calls the supplied function with a new scope of the allocator.
    ///
    /// # Safety
    ///
    /// Any of the member functions of ScopedAllocator will panic if called
    /// on the outer allocator inside the scope.
    pub fn scope<F, U>(&self, f: F) -> U where F: FnMut(&ScopedAllocator) -> U {
        self.ensure_not_scoped();
        let mut f = f;
        let old = self.current.get();
        let alloc = ScopedAllocator {
            current: self.current.clone(),
            end: self.end,
            start: self.start,
        };
        
        // set the current pointer to null as a flag to indicate
        // that this allocator is being scoped.
        self.current.set(ptr::null_mut());
        let u = f(&alloc);
        self.current.set(old);
        
        mem::forget(alloc);
        u
    }

    fn ensure_not_scoped(&self) {
        debug_assert!(
            !self.current.get().is_null(), 
            "Called method on currently scoped allocator"
        );
    }
}

impl Allocator for ScopedAllocator {

    /// Attempts to allocate some bytes directly.
    /// Returns either a pointer to the start of the allocated block or nothing.
    ///
    /// # Panics
    ///
    /// Panics if this is called on a currently scoped allocator.
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<*mut u8, ()> {
        self.ensure_not_scoped();
        let current_ptr = self.current.get();
        let aligned_ptr = ((current_ptr as usize + align - 1) & !(align - 1)) as *mut u8;
        let end_ptr = aligned_ptr.offset(size as isize);

        if end_ptr > self.end {
            Err(())
        } else {
            self.current.set(end_ptr);
            Ok(aligned_ptr)
        }
    }

    #[allow(unused_variables)]
    unsafe fn deallocate_raw(&self, ptr: *mut u8, size: usize, align: usize) {
        self.ensure_not_scoped();
        // no op for this. The memory gets reused when the scope is cleared.
    }
}

impl Drop for ScopedAllocator {
    /// Drops the `ScopedAllocator`
    fn drop(&mut self) {
        let size = self.end as usize - self.start as usize;
        // if the allocator is scoped, the memory will be freed by the child.
        if !self.current.get().is_null() && size > 0 { 
            unsafe { heap::deallocate(self.start, size, mem::align_of::<usize>()) }
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
        let alloc = ScopedAllocator::new(4);
        alloc.scope(|_inner| {
            // using outer allocator is dangerous and should fail.
            let _val = alloc.allocate(1i32).ok().unwrap();
        })
    }

    #[test]
    fn test_unsizing() {
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) { println!("Boom") }
        }

        let alloc = ScopedAllocator::new(4);
        let my_foo: Allocated<Any, _> = alloc.allocate(Bomb).ok().unwrap();
        let _: Allocated<Bomb, _> = my_foo.downcast().ok().unwrap();
    }
}