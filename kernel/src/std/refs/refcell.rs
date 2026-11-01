use core::cell::{Cell, UnsafeCell};
use core::ops::{Deref, DerefMut};

pub struct RefCell<T> {
    borrow_state: Cell<isize>,
    value: UnsafeCell<T>,
}

impl<T> RefCell<T> {
    pub const fn new(value: T) -> Self {
        Self {
            borrow_state: Cell::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub fn borrow(&self) -> Ref<'_, T> {
        let current = self.borrow_state.get();
        if current < 0 { panic!("RefCell is already &mut borrowed"); }
        self.borrow_state.set(current + 1);
        Ref { cell: self }
    }

    pub fn borrow_mut(&self) -> RefMut<'_, T> {
        let current = self.borrow_state.get();
        if current != 0 { panic!("Already borrowed!"); }
        self.borrow_state.set(-1);
        RefMut { cell: self }
    }
}

pub struct Ref<'a, T> { cell: &'a RefCell<T> }
pub struct RefMut<'a, T> { cell: &'a RefCell<T> }

impl<'a, T> Deref for Ref<'a, T> {
    type Target = T;
    
    fn deref(&self) -> &Self::Target { unsafe { &*self.cell.value.get() } }
}

impl<'a, T> Drop for Ref<'a, T> {
    fn drop(&mut self) {
        self.cell.borrow_state.set(self.cell.borrow_state.get() - 1);
    }
}

impl<'a, T> Deref for RefMut<'a, T> {
    type Target = T;
    
    fn deref(&self) -> &Self::Target { unsafe { &*self.cell.value.get() } }
}

impl<'a, T> DerefMut for RefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.cell.value.get() } }
}

impl<'a, T> Drop for RefMut<'a, T> {
    fn drop(&mut self) {
        self.cell.borrow_state.set(0);
    }
}