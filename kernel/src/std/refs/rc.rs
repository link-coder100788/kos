use core::alloc::Layout;
use core::cell::Cell;
use core::ptr::NonNull;
use alloc::alloc::{alloc, dealloc};
use core::ops::Deref;

pub struct RcBox<T> {
    strong: Cell<usize>,
    value: T,
}

pub struct Rc<T> {
    ptr: NonNull<RcBox<T>>,
}

impl<T> Rc<T> {
    pub fn new(value: T) -> Self {
        let layout = Layout::new::<RcBox<T>>();
        unsafe {
            let raw_ptr = alloc(layout) as *mut RcBox<T>;
            let ptr = NonNull::new(raw_ptr).expect("Allocation failed");
            core::ptr::write(ptr.as_ptr(), RcBox { strong: Cell::new(1), value });
            Self { ptr }
        }
    }

    pub fn strong_count(&self) -> usize {
        unsafe { self.ptr.as_ref().strong.get() }
    }
}

impl<T> Clone for Rc<T> {
    fn clone(&self) -> Self {
        let rc_box = unsafe { self.ptr.as_ref() };
        let count = rc_box.strong.get();
        if count == usize::MAX { panic!("Rc ref count overflow"); }
        rc_box.strong.set(count + 1);
        Self { ptr: self.ptr }
    }
}

impl<T> Deref for Rc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &self.ptr.as_ref().value }
    }
}

impl<T> Drop for Rc<T> {
    fn drop(&mut self) {
        unsafe {
            let rc_box = self.ptr.as_ref();
            let count = rc_box.strong.get();
            if count == 1 {
                let value_ptr = core::ptr::addr_of_mut!((*self.ptr.as_ptr()).value);
                core::ptr::drop_in_place(value_ptr);
                let layout = Layout::new::<RcBox<T>>();
                dealloc(self.ptr.as_ptr() as *mut u8, layout);
            } else {
                rc_box.strong.set(count - 1);
            }
        }
    }
}

impl<T> !Send for Rc<T> {}
impl<T> !Sync for Rc<T> {}
