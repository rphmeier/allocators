use std::cell::Cell;
use std::mem;
use std::ptr;

use super::{Allocator, AllocatorError, HeapAllocator, HEAP};

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
            Ok(start) => Ok(ScopedAllocator {
                allocator: alloc,
                current: Cell::new(start),
                end: unsafe { start.offset(size as isize) },
                root: true,
                start: start,
            }),
            Err(err) => Err(err),
        }
    }

    /// Calls the supplied function with a new scope of the allocator.
    ///
    /// Returns the result of the closure or an error if this allocator
    /// has already been scoped.
    pub fn scope<F, U>(&self, f: F) -> Result<U, ()>
        where F: FnMut(&Self) -> U
    {
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
            return Err(AllocatorError::AllocatorSpecific("Called allocate on already scoped \
                                                          allocator."
                                                             .into()))
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
        // no op for this unless this is the last allocation.
        // The memory gets reused when the scope is cleared.
        let current_ptr = self.current.get();
        if !self.is_scoped() && ptr.offset(size as isize) == current_ptr {
            self.current.set(ptr);
        }
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

    use super::super::*;

    #[test]
    #[should_panic]
    fn use_outer() {
        let alloc = ScopedAllocator::new(4).unwrap();
        let mut outer_val = alloc.allocate_val(0i32).ok().unwrap();
        alloc.scope(|_inner| {
            // using outer allocator is dangerous and should fail.
                 outer_val = alloc.allocate_val(1i32).ok().unwrap();
             })
             .unwrap();
    }

    #[test]
    fn unsizing() {
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) {
                println!("Boom")
            }
        }

        let alloc = ScopedAllocator::new(4).unwrap();
        let my_foo: Allocated<Any, _> = alloc.allocate_val(Bomb).ok().unwrap();
        let _: Allocated<Bomb, _> = my_foo.downcast().ok().unwrap();
    }

    #[test]
    fn scope_scope() {
        let alloc = ScopedAllocator::new(64).unwrap();
        let _ = alloc.allocate_val(0).ok().unwrap();
        alloc.scope(|inner| {
                 let _ = inner.allocate_val(32);
                 inner.scope(|bottom| {
                          let _ = bottom.allocate_val(23);
                      })
                      .unwrap();
             })
             .unwrap();
    }

    #[test]
    fn out_of_memory() {
        // allocate more memory than the allocator has.
        let alloc = ScopedAllocator::new(0).unwrap();
        let (err, _) = alloc.allocate_val(1i32).err().unwrap();
        assert_eq!(err, AllocatorError::OutOfMemory);
    }

    #[test]
    fn placement_in() {
        let alloc = ScopedAllocator::new(8_000_000).unwrap();
        // this would smash the stack otherwise.
        let _big = in alloc.allocate().unwrap() { [0u8; 8_000_000] };
    }
}
