//! This module contains some composable building blocks to build allocator chains.

use std::cell::RefCell;

use super::{Allocator, AllocatorError, Block, BlockOwner};

/// This allocator always fails.
/// It will panic if you try to deallocate with it.
pub struct NullAllocator;

unsafe impl Allocator for NullAllocator {
    unsafe fn allocate_raw(&self, _size: usize, _align: usize) -> Result<Block, AllocatorError> {
        Err(AllocatorError::OutOfMemory)
    }

    unsafe fn reallocate_raw<'a>(&'a self, block: Block<'a>, _new_size: usize) -> Result<Block<'a>, (AllocatorError, Block<'a>)> {
        Err((AllocatorError::OutOfMemory, block))
    }

    unsafe fn deallocate_raw(&self, _block: Block) {
        panic!("Attempted to deallocate using null allocator.")
    }
}

impl BlockOwner for NullAllocator {
    fn owns_block(&self, _block: &Block) -> bool {
        false
    }
}

/// This allocator has a main and a fallback allocator.
/// It will always attempt to allocate first with the main allocator,
/// and second with the fallback.
pub struct Fallback<M: BlockOwner, F: BlockOwner> {
    main: M,
    fallback: F,
}

impl<M: BlockOwner, F: BlockOwner> Fallback<M, F> {
    /// Create a new `Fallback`
    pub fn new(main: M, fallback: F) -> Self {
        Fallback {
            main: main,
            fallback: fallback,
        }
    }
}

unsafe impl<M: BlockOwner, F: BlockOwner> Allocator for Fallback<M, F> {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, AllocatorError> {
        match self.main.allocate_raw(size, align) {
            Ok(block) => Ok(block),
            Err(_) => self.fallback.allocate_raw(size, align),
        }
    }

    unsafe fn reallocate_raw<'a>(&'a self, block: Block<'a>, new_size: usize) -> Result<Block<'a>, (AllocatorError, Block<'a>)> {
        if self.main.owns_block(&block) {
            self.main.reallocate_raw(block, new_size)
        } else if self.fallback.owns_block(&block) {
            self.fallback.reallocate_raw(block, new_size)
        } else {
            Err((AllocatorError::AllocatorSpecific("Neither fallback nor main owns this block.".into()), block))
        }
    }

    unsafe fn deallocate_raw(&self, block: Block) {
        if self.main.owns_block(&block) {
            self.main.deallocate_raw(block);
        } else if self.fallback.owns_block(&block) {
            self.fallback.deallocate_raw(block);
        }
    }
}

impl<M: BlockOwner, F: BlockOwner> BlockOwner for Fallback<M, F> {
    fn owns_block(&self, block: &Block) -> bool {
        self.main.owns_block(block) || self.fallback.owns_block(block)
    }
}

/// Something that logs an allocator's activity.
/// In practice, this may be an output stream,
/// a data collector, or seomthing else entirely.
pub trait ProxyLogger {
    /// Called after a successful allocation.
    fn allocate_success(&mut self, block: &Block);
    /// Called after a failed allocation.
    fn allocate_fail(&mut self, err: &AllocatorError, size: usize, align: usize);

    /// Called when deallocating a block.
    fn deallocate(&mut self, block: &Block);

    /// Called after a successful reallocation.
    fn reallocate_success(&mut self, old_block: &Block, new_block: &Block);
    /// Called after a failed reallocation.
    fn reallocate_fail(&mut self, err: &AllocatorError, block: &Block, req_size: usize);
}

/// This wraps an allocator and a logger, logging all allocations
/// and deallocations.
pub struct Proxy<A, L> {
    alloc: A,
    logger: RefCell<L>,
}

impl<A: Allocator, L: ProxyLogger> Proxy<A, L> {
    /// Create a new proxy allocator.
    pub fn new(alloc: A, logger: L) -> Self {
        Proxy {
            alloc: alloc,
            logger: RefCell::new(logger),
        }
    }
}

unsafe impl<A: Allocator, L: ProxyLogger> Allocator for Proxy<A, L> {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, AllocatorError> {
        let mut logger = self.logger.borrow_mut();
        match self.alloc.allocate_raw(size, align) {
            Ok(block) => {
                logger.allocate_success(&block);
                Ok(block)
            }
            Err(err) => {
                logger.allocate_fail(&err, size, align);
                Err(err)
            }
        }
    }

    unsafe fn reallocate_raw<'a>(&'a self, block: Block<'a>, new_size: usize) -> Result<Block<'a>, (AllocatorError, Block<'a>)> {
        let mut logger = self.logger.borrow_mut();
        let old_copy = Block::new(block.ptr(), block.size(), block.align());

        match self.alloc.reallocate_raw(block, new_size) {
            Ok(new_block) => {
                logger.reallocate_success(&old_copy, &new_block);
                Ok(new_block)
            }
            Err((err, old)) => {
                logger.reallocate_fail(&err, &old, new_size);
                Err((err, old))
            }
        }
    }

    unsafe fn deallocate_raw(&self, block: Block) {
        let mut logger = self.logger.borrow_mut();
        logger.deallocate(&block);
        self.alloc.deallocate_raw(block);
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    #[should_panic]
    fn null_allocate() {
        let alloc = NullAllocator;
        alloc.allocate(1i32).unwrap();
    }
}
