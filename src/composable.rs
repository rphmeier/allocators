//! This module contains some composable building blocks to build allocator chains.

use std::cell::RefCell;
use std::error::Error;
use std::io::Write;

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

/// This wraps an allocator and a writer, logging all allocations
/// and deallocations.
pub struct Proxy<A, W> {
    alloc: A,
    writer: RefCell<W>,
}

impl<A: Allocator, W: Write> Proxy<A, W> {
    /// Create a new proxy allocator.
    pub fn new(alloc: A, writer: W) -> Self {
        Proxy {
            alloc: alloc,
            writer: RefCell::new(writer),
        }
    }
}

unsafe impl<A: Allocator, W: Write> Allocator for Proxy<A, W> {
    #[allow(unused_must_use)]
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, AllocatorError> {
        let mut writer = self.writer.borrow_mut();
        match self.alloc.allocate_raw(size, align) {
            Ok(block) => {
                writeln!(writer,
                         "Successfully allocated {} bytes with align {}",
                         size,
                         align);
                writeln!(writer, "Returned pointer is {:p}", block.ptr());
                Ok(block)
            }
            Err(err) => {
                writeln!(writer, "Failed to allocate {} bytes.", size);
                writeln!(writer, "Error: {}", err.description());
                Err(err)
            }
        }
    }

    #[allow(unused_must_use)]
    unsafe fn reallocate_raw<'a>(&'a self, block: Block<'a>, new_size: usize) -> Result<Block<'a>, (AllocatorError, Block<'a>)> {
        let mut writer = self.writer.borrow_mut();
        let (old_ptr, old_size) = (block.ptr(), block.size());

        match self.alloc.reallocate_raw(block, new_size) {
            Ok(new_block) => {
                writeln!(writer,
                        "Successfully reallocated block at pointer {:p}",
                        old_ptr);
                writeln!(writer,
                        "Old size: {}, new size: {}",
                        old_size,
                        new_size);
                Ok(new_block)
            }
            Err((err, old)) => {
                writeln!(writer,
                        "Failed to reallocate block at pointer {:p}",
                        old_ptr);
                writeln!(writer,
                        "Old size: {}, new size: {}",
                        old_size,
                        new_size);
                writeln!(writer, "Error: {}", err.description());
                Err((err, old))
            }
        }
    }

    #[allow(unused_must_use)]
    unsafe fn deallocate_raw(&self, block: Block) {
        let mut writer = self.writer.borrow_mut();
        write!(writer,
               "Deallocating block at pointer {:p} with size {} and align {}",
               block.ptr(),
               block.size(),
               block.align());
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
