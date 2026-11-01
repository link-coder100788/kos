use crate::println;

pub fn debug_allocations() {
    println!("Allocations: ");
    let last = crate::mm::allocator::ALLOCATOR.last_allocations();
    for (i, alloc) in last.iter().enumerate() {
        println!("  {i}: {alloc:?}");
    }
}

