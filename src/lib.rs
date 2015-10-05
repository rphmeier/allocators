#![feature(alloc, heap_api, ptr_as_ref)]

use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;

use alloc::heap;

extern crate alloc;

/// An item allocated by a custom allocator.
pub struct Allocated<'a, T: 'a> {
    item: &'a mut T,
}

impl<'a, T> Deref for Allocated<'a, T> {
    type Target = T;

    fn deref<'b>(&'b self) -> &'b T {
        &*self.item
    }
}

impl<'a, T> DerefMut for Allocated<'a, T> {
    fn deref_mut<'b>(&'b mut self) -> &'b mut T {
        self.item
    }
}

impl<'a, T> Drop for Allocated<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // could also use ptr::read_and_drop
        unsafe { let _ = ptr::read(self.item as *mut T); }
    }
}

/// A scoped linear allocator
pub struct ScopedAllocator {
    current: *mut u8,
    end: *mut u8,
    start: *mut u8,
}

impl ScopedAllocator {
    /// Creates a new `ScopedAllocator` backed by a memory buffer of given size.
    pub fn new(size: usize) -> ScopedAllocator {
        // Create a memory buffer with the desired size and maximal align.
        let start = if size != 0 {
            unsafe { heap::allocate(size, mem::align_of::<usize>()) }
        } else {
            heap::EMPTY as *mut u8
        };

        if start.is_null() {
            panic!("Out of memory!");
        }

        ScopedAllocator {
            current: start,
            end: unsafe { start.offset(size as isize) },
            start: start,
        }

    }

    /// Attempts to allocate space for the T supplied to it.
    ///
    /// This function is most definitely not thread-safe.
    /// This either returns the allocated object, or returns `val` back on failure.
    pub fn allocate<'a, T>(&'a self, val: T) -> Result<Allocated<'a, T>, T> {
        match unsafe { self.allocate_raw(mem::size_of::<T>(), mem::align_of::<T>()) } {
            Ok(ptr) => {
                let item = ptr as *mut T;
                unsafe { ptr::write(item, val) };

                Ok(Allocated {
                    item: unsafe { item.as_mut().unwrap() },
                })
            }
            Err(_) => Err(val)
        }
    }

    /// Attempts to allocate some bytes directly.
    /// Returns either a pointer to the start of the block or nothing.
    pub unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<*mut u8, ()> {
        let current_ptr = self.current;
        let aligned_ptr = ((current_ptr as usize + align - 1) & !(align - 1)) as *mut u8;
        let end_ptr = aligned_ptr.offset(size as isize);

        if end_ptr > self.end {
            Err(())
        } else {
            self.set_current(end_ptr);
            Ok(aligned_ptr)
        }
    }

    /// Calls the supplied function with a new scope of the allocator.
    ///
    /// Values allocated in the scope cannot be moved outside.
    #[inline]
    pub fn scope<F, U>(&self, f: F) -> U where F: FnMut() -> U {
        let mut f = f;
        let old = self.current;
        let u = f();
        self.set_current(old);
        u
    }

    #[inline(always)]
    fn set_current(&self, new: *mut u8) {
        let ptr = &self.current as *const _ as *mut _;
        unsafe {*ptr = new }
    }
}

impl Drop for ScopedAllocator {
    /// Drops the `ScopedAllocator`
    #[inline]
    fn drop(&mut self) {
        let size = self.end as usize - self.start as usize;
        if size > 0 { 
            unsafe { heap::deallocate(self.start, size, mem::align_of::<usize>()) }
        }
    }
}

#[test]
fn it_works() {
    struct Bomb(i32);
    impl Drop for Bomb {
        fn drop(&mut self) { println!("Boom! {}", self.0) }
    }

    let alloc = ScopedAllocator::new(64);
    let _my_int = alloc.allocate(23).ok().unwrap();
    alloc.scope(|| {
        //gets dropped when this scope ends.
        let _bomb = {
            alloc.allocate(Bomb(1)).ok().unwrap()
        };
    });
}