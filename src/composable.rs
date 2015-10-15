/// This module contains some composable building blocks to build allocator chains.

use super::{Allocator, AllocatorError, Block, OwningAllocator};

/// This allocator always fails.
/// It will panic if you try to deallocate with it.
pub struct NullAllocator;

unsafe impl Allocator for NullAllocator {
    unsafe fn allocate_raw(&self, _size: usize, _align: usize) -> Result<Block, AllocatorError> {
        Err(AllocatorError::OutOfMemory)
    }

    unsafe fn deallocate_raw(&self, _blk: Block) {
        panic!("Attempted to deallocate using null allocator.")
    }
}

impl OwningAllocator for NullAllocator {
    fn owns_block(&self, _blk: &Block) -> bool {
        false
    }
}

/// This allocator has a main and a fallback allocator.
/// It will always attempt to allocate first with the main allocator,
/// and second with the fallback.
pub struct FallbackAllocator<M: OwningAllocator, F: OwningAllocator> {
    main: M,
    fallback: F,
}

impl<M: OwningAllocator, F: OwningAllocator> FallbackAllocator<M, F> {
    /// Create a new `FallbackAllocator`
    pub fn new(main: M, fallback: F) -> Self {
        FallbackAllocator {
            main: main,
            fallback: fallback,
        }
    }
}

unsafe impl<M: OwningAllocator, F: OwningAllocator> Allocator for FallbackAllocator<M, F> {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, AllocatorError> {
        match self.main.allocate_raw(size, align) {
            Ok(blk) => Ok(blk),
            Err(_) => self.fallback.allocate_raw(size, align)
        }
    }

    unsafe fn deallocate_raw(&self, blk: Block) {
        if self.main.owns_block(&blk) {
            self.main.deallocate_raw(blk);
        } else if self.fallback.owns_block(&blk) {
            self.fallback.deallocate_raw(blk);
        }
    }
}

impl<M: OwningAllocator, F: OwningAllocator> OwningAllocator for FallbackAllocator<M, F> {
    fn owns_block(&self, blk: &Block) -> bool {
        self.main.owns_block(blk) || self.fallback.owns_block(blk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::*;

    #[test]
    #[should_panic]
    fn null_allocate() {
        let alloc = NullAllocator;
        alloc.allocate(1i32).unwrap();
    }
}