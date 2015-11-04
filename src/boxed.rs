use std::any::Any;
use std::borrow::{Borrow, BorrowMut};
use std::marker::{PhantomData, Unsize};
use std::mem;
use std::ops::{CoerceUnsized, Deref, DerefMut, InPlace, Placer};
use std::ops::Place as StdPlace;
use std::ptr::Unique;

use super::{Allocator, AllocatorError, Block};
/// An item allocated by a custom allocator.
pub struct AllocBox<'a, T: 'a + ?Sized, A: 'a + ?Sized + Allocator> {
    item: Unique<T>,
    size: usize,
    align: usize,
    allocator: &'a A,
}

impl<'a, T: ?Sized, A: ?Sized + Allocator> AllocBox<'a, T, A> {
    /// Consumes this allocated value, yielding the value it manages.
    pub fn take(self) -> T where T: Sized {
        let val = unsafe { ::std::ptr::read(*self.item) };
        let block = Block::new(*self.item as *mut u8, self.size, self.align);
        unsafe { self.allocator.deallocate_raw(block) };
        mem::forget(self);
        val
    }

    /// Gets a handle to the block of memory this manages.
    pub unsafe fn as_block(&self) -> Block {
        Block::new(*self.item as *mut u8, self.size, self.align)
    }
}

impl<'a, T: ?Sized, A: ?Sized + Allocator> Deref for AllocBox<'a, T, A> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { self.item.get() }
    }
}

impl<'a, T: ?Sized, A: ?Sized + Allocator> DerefMut for AllocBox<'a, T, A> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.item.get_mut() }
    }
}

// AllocBox can store trait objects!
impl<'a, T: ?Sized + Unsize<U>, U: ?Sized, A: ?Sized + Allocator> CoerceUnsized<AllocBox<'a, U, A>> for AllocBox<'a, T, A> {}

impl<'a, A: ?Sized + Allocator> AllocBox<'a, Any, A> {
    /// Attempts to downcast this `AllocBox` to a concrete type.
    pub fn downcast<T: Any>(self) -> Result<AllocBox<'a, T, A>, AllocBox<'a, Any, A>> {
        use std::raw::TraitObject;
        if self.is::<T>() {
            let obj: TraitObject = unsafe { mem::transmute::<*mut Any, TraitObject>(*self.item) };
            let new_allocated = AllocBox {
                item: unsafe { Unique::new(obj.data as *mut T) },
                size: self.size,
                align: self.align,
                allocator: self.allocator,
            };
            mem::forget(self);
            Ok(new_allocated)
        } else {
            Err(self)
        }
    }
}

impl<'a, T: ?Sized, A: ?Sized + Allocator> Borrow<T> for AllocBox<'a, T, A> {
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<'a, T: ?Sized, A: ?Sized + Allocator> BorrowMut<T> for AllocBox<'a, T, A> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<'a, T: ?Sized, A: ?Sized + Allocator> Drop for AllocBox<'a, T, A> {
    #[inline]
    fn drop(&mut self) {
        use std::intrinsics::drop_in_place;
        unsafe {
            drop_in_place(*self.item);
            self.allocator.deallocate_raw(Block::new(*self.item as *mut u8, self.size, self.align));
        }

    }
}


pub fn make_place<A: ?Sized + Allocator, T>(alloc: &A) -> Result<Place<T, A>, super::AllocatorError> {
    let (size, align) = (mem::size_of::<T>(), mem::align_of::<T>());
    match unsafe { alloc.allocate_raw(size, align) } {
        Ok(block) => {
            Ok(Place {
                allocator: alloc,
                block: block,
                _marker: PhantomData,
            })
        }
        Err(e) => Err(e),
    }
}

/// A place for allocating into.
/// This is only used for in-place allocation,
/// e.g. `let val = in (alloc.make_place().unwrap()) { EXPR }`
pub struct Place<'a, T: 'a, A: 'a + ?Sized + Allocator> {
    allocator: &'a A,
    block: Block<'a>,
    _marker: PhantomData<T>,
}

impl<'a, T: 'a, A: 'a + ?Sized + Allocator> Placer<T> for Place<'a, T, A> {
    type Place = Self;
    fn make_place(self) -> Self {
        self
    }
}

impl<'a, T: 'a, A: 'a + ?Sized + Allocator> InPlace<T> for Place<'a, T, A> {
    type Owner = AllocBox<'a, T, A>;
    unsafe fn finalize(self) -> Self::Owner {
        let allocated = AllocBox {
            item: Unique::new(self.block.ptr() as *mut T),
            size: self.block.size(),
            align: self.block.align(),
            allocator: self.allocator,
        };

        mem::forget(self);
        allocated
    }
}

impl<'a, T: 'a, A: 'a + ?Sized + Allocator> StdPlace<T> for Place<'a, T, A> {
    fn pointer(&mut self) -> *mut T {
        self.block.ptr() as *mut T
    }
}

impl<'a, T: 'a, A: 'a + ?Sized + Allocator> Drop for Place<'a, T, A> {
    #[inline]
    fn drop(&mut self) {
        // almost identical to AllocBox::Drop, but we don't drop
        // the value in place. If the finalize
        // method was never called, the expression
        // to create the value failed and the memory at the
        // pointer is still uninitialized, which we don't want to drop.
        unsafe {
            self.allocator.deallocate_raw(mem::replace(&mut self.block, Block::empty()));
        }

    }
}