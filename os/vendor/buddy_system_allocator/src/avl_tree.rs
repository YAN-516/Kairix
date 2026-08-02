//! Intrusive AVL tree used by the heap free lists.
//!
//! Free blocks carry their own tree node, so maintaining the address index
//! never allocates memory (which would recurse back into the heap allocator).

use core::cmp::max;
use core::mem::size_of;
use core::ptr;

#[repr(C)]
struct TreeNode {
    left: *mut TreeNode,
    right: *mut TreeNode,
    height: usize,
}

/// Smallest block that can carry an intrusive tree node.
pub const MIN_BLOCK_SIZE: usize = size_of::<TreeNode>().next_power_of_two();

/// Address-ordered intrusive AVL tree.
#[derive(Copy, Clone)]
pub struct AvlTree {
    root: *mut TreeNode,
}

unsafe impl Send for AvlTree {}

impl AvlTree {
    /// Creates an empty tree.
    pub const fn new() -> Self {
        Self {
            root: ptr::null_mut(),
        }
    }

    /// Returns whether the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.root.is_null()
    }

    /// Inserts a free block, rejecting duplicate addresses.
    pub unsafe fn insert(&mut self, block: *mut usize) {
        assert!(
            !self.contains(block as usize),
            "buddy duplicate free-list insertion: ptr={:#x}",
            block as usize
        );
        let node = block.cast::<TreeNode>();
        unsafe {
            node.write(TreeNode {
                left: ptr::null_mut(),
                right: ptr::null_mut(),
                height: 1,
            });
            self.root = insert_node(self.root, node);
        }
    }

    /// Removes and returns the lowest-addressed block.
    pub fn pop(&mut self) -> Option<*mut usize> {
        if self.root.is_null() {
            return None;
        }
        let (root, node) = unsafe { remove_min(self.root) };
        self.root = root;
        Some(node.cast::<usize>())
    }

    /// Removes an exact block address in O(log n).
    pub fn remove(&mut self, block: usize) -> Option<*mut usize> {
        let (root, removed) = unsafe { remove_node(self.root, block) };
        self.root = root;
        removed.map(|node| node.cast::<usize>())
    }

    /// Looks up an exact block address in O(log n).
    pub fn contains(&self, block: usize) -> bool {
        let mut current = self.root;
        while !current.is_null() {
            let address = current as usize;
            if block < address {
                current = unsafe { (*current).left };
            } else if block > address {
                current = unsafe { (*current).right };
            } else {
                return true;
            }
        }
        false
    }
}

#[inline]
unsafe fn height(node: *mut TreeNode) -> usize {
    if node.is_null() {
        0
    } else {
        unsafe { (*node).height }
    }
}

#[inline]
unsafe fn update_height(node: *mut TreeNode) {
    unsafe {
        (*node).height = 1 + max(height((*node).left), height((*node).right));
    }
}

#[inline]
unsafe fn rotate_left(root: *mut TreeNode) -> *mut TreeNode {
    unsafe {
        let pivot = (*root).right;
        debug_assert!(!pivot.is_null());
        (*root).right = (*pivot).left;
        (*pivot).left = root;
        update_height(root);
        update_height(pivot);
        pivot
    }
}

#[inline]
unsafe fn rotate_right(root: *mut TreeNode) -> *mut TreeNode {
    unsafe {
        let pivot = (*root).left;
        debug_assert!(!pivot.is_null());
        (*root).left = (*pivot).right;
        (*pivot).right = root;
        update_height(root);
        update_height(pivot);
        pivot
    }
}

unsafe fn rebalance(root: *mut TreeNode) -> *mut TreeNode {
    if root.is_null() {
        return root;
    }
    unsafe {
        update_height(root);
        let left_height = height((*root).left);
        let right_height = height((*root).right);
        if left_height > right_height + 1 {
            let left = (*root).left;
            if height((*left).right) > height((*left).left) {
                (*root).left = rotate_left(left);
            }
            rotate_right(root)
        } else if right_height > left_height + 1 {
            let right = (*root).right;
            if height((*right).left) > height((*right).right) {
                (*root).right = rotate_right(right);
            }
            rotate_left(root)
        } else {
            root
        }
    }
}

unsafe fn insert_node(root: *mut TreeNode, node: *mut TreeNode) -> *mut TreeNode {
    if root.is_null() {
        return node;
    }
    unsafe {
        if (node as usize) < (root as usize) {
            (*root).left = insert_node((*root).left, node);
        } else if (node as usize) > (root as usize) {
            (*root).right = insert_node((*root).right, node);
        } else {
            unreachable!("duplicate AVL insertion was rejected before node initialization");
        }
        rebalance(root)
    }
}

/// Detaches the minimum node and returns `(new_root, minimum)`.
unsafe fn remove_min(root: *mut TreeNode) -> (*mut TreeNode, *mut TreeNode) {
    unsafe {
        if (*root).left.is_null() {
            return ((*root).right, root);
        }
        let (left, minimum) = remove_min((*root).left);
        (*root).left = left;
        (rebalance(root), minimum)
    }
}

unsafe fn remove_node(
    root: *mut TreeNode,
    address: usize,
) -> (*mut TreeNode, Option<*mut TreeNode>) {
    if root.is_null() {
        return (root, None);
    }
    unsafe {
        if address < root as usize {
            let (left, removed) = remove_node((*root).left, address);
            (*root).left = left;
            (rebalance(root), removed)
        } else if address > root as usize {
            let (right, removed) = remove_node((*root).right, address);
            (*root).right = right;
            (rebalance(root), removed)
        } else {
            let left = (*root).left;
            let right = (*root).right;
            if right.is_null() {
                (left, Some(root))
            } else {
                let (new_right, successor) = remove_min(right);
                (*successor).left = left;
                (*successor).right = new_right;
                (rebalance(successor), Some(root))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AvlTree, MIN_BLOCK_SIZE};
    use std::alloc::{alloc, dealloc, Layout};

    #[test]
    fn insert_lookup_remove_many_addresses() {
        const BLOCKS: usize = 1024;
        let layout = Layout::from_size_align(BLOCKS * MIN_BLOCK_SIZE, MIN_BLOCK_SIZE).unwrap();
        let backing = unsafe { alloc(layout) };
        assert!(!backing.is_null());
        let mut tree = AvlTree::new();

        for index in (0..BLOCKS).rev() {
            unsafe { tree.insert(backing.add(index * MIN_BLOCK_SIZE).cast()) };
        }
        for index in 0..BLOCKS {
            assert!(tree.contains(unsafe { backing.add(index * MIN_BLOCK_SIZE) } as usize));
        }
        for index in (0..BLOCKS).step_by(2) {
            let address = unsafe { backing.add(index * MIN_BLOCK_SIZE) } as usize;
            assert_eq!(
                tree.remove(address).map(|node| node as usize),
                Some(address)
            );
        }
        for index in 0..BLOCKS {
            let address = unsafe { backing.add(index * MIN_BLOCK_SIZE) } as usize;
            assert_eq!(tree.contains(address), index % 2 == 1);
        }
        let mut previous = 0;
        while let Some(node) = tree.pop() {
            assert!(node as usize > previous);
            previous = node as usize;
        }
        assert!(tree.is_empty());
        unsafe { dealloc(backing, layout) };
    }
}
