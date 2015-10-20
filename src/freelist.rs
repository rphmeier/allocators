//! A Free List allocator.

use std::cell::Cell;
use std::mem;
use std::ptr;

use super::{Allocator, AllocatorError, Block, HeapAllocator, HEAP};

/// A `FreeList` allocator manages a list of free memory blocks of uniform size.
/// Whenever a block is requested, it returns the first free block.
pub struct FreeList<'a, A: 'a + Allocator> {
    alloc: &'a A,
    block_size: usize,
    free_list: Cell<*mut u8>,
}

impl FreeList<'static, HeapAllocator> {
    /// Creates a new `FreeList` backed by the heap. `block_size` must be greater
    /// than or equal to the size of a pointer.
    pub fn new(block_size: usize, num_blocks: usize) -> Result<Self, AllocatorError> {
        FreeList::new_from(HEAP, block_size, num_blocks)
    }
}
impl<'a, A: 'a + Allocator> FreeList<'a, A> {
    /// Creates a new `FreeList` backed by another allocator. `block_size` must be greater
    /// than or equal to the size of a pointer.
    pub fn new_from(alloc: &'a A,
                    block_size: usize,
                    num_blocks: usize)
                    -> Result<Self, AllocatorError> {
        if block_size < mem::size_of::<*mut u8>() {
            return Err(AllocatorError::AllocatorSpecific("Block size too small.".into()));
        }

        let mut free_list = ptr::null_mut();

        // allocate each block with maximal alignment.
        for _ in 0..num_blocks {

            match unsafe { alloc.allocate_raw(block_size, mem::align_of::<*mut u8>()) } {
                Ok(block) => {
                    let ptr: *mut *mut u8 = block.ptr() as *mut *mut u8;
                    unsafe { *ptr = free_list }
                    free_list = block.ptr();
                }
                Err(err) => {
                    // destructor cleans up after us.
                    drop(FreeList {
                        alloc: alloc,
                        block_size: block_size,
                        free_list: Cell::new(free_list),
                    });

                    return Err(err);
                }
            }
        }

        Ok(FreeList {
            alloc: alloc,
            block_size: block_size,
            free_list: Cell::new(free_list),
        })
    }
}

unsafe impl<'a, A: 'a + Allocator> Allocator for FreeList<'a, A> {
    unsafe fn allocate_raw(&self, size: usize, align: usize) -> Result<Block, AllocatorError> {
        if size == 0 {
            return Ok(Block::empty());
        } else if size > self.block_size {
            return Err(AllocatorError::OutOfMemory);
        }

        if align > mem::align_of::<*mut u8>() {
            return Err(AllocatorError::UnsupportedAlignment);
        }

        let free_list = self.free_list.get();
        if !free_list.is_null() {
            let next_block = *(free_list as *mut *mut u8);
            self.free_list.set(next_block);

            Ok(Block::new(free_list, size, align))
        } else {
            Err(AllocatorError::OutOfMemory)
        }
    }

    unsafe fn reallocate_raw<'b>(&'b self, block: Block<'b>, new_size: usize) -> Result<Block<'b>, (AllocatorError, Block<'b>)> {
        if new_size == 0 {
            Ok(Block::empty())
        } else if block.is_empty() {
            Err((AllocatorError::UnsupportedAlignment, block))
        } else if new_size <= self.block_size {
            Ok(Block::new(block.ptr(), new_size, block.align()))
        } else {
            Err((AllocatorError::OutOfMemory, block))
        }
    }

    unsafe fn deallocate_raw(&self, blk: Block) {
        if !blk.is_empty() {
            let first = self.free_list.get();
            let ptr = blk.ptr();
            *(ptr as *mut *mut u8) = first;
            self.free_list.set(ptr);
        }
    }
}

impl<'a, A: 'a + Allocator> Drop for FreeList<'a, A> {
    fn drop(&mut self) {
        let mut free_list = self.free_list.get();
        //free all the blocks in the list.
        while !free_list.is_null() {
            unsafe {
                let next = *(free_list as *mut *mut u8);
                self.alloc.deallocate_raw(Block::new(free_list,
                                                     self.block_size,
                                                     mem::align_of::<*mut u8>()));
                free_list = next;
            }
        }
    }
}

unsafe impl<'a, A: 'a + Allocator + Sync> Send for FreeList<'a, A> {}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn it_works() {
        let alloc = FreeList::new(1024, 64).ok().unwrap();
        let mut blocks = Vec::new();
        for _ in 0..64 {
            blocks.push(alloc.allocate([0u8; 1024]).ok().unwrap());
        }
        assert!(alloc.allocate([0u8; 1024]).is_err());
        drop(blocks);
        assert!(alloc.allocate([0u8; 1024]).is_ok());
    }
}
