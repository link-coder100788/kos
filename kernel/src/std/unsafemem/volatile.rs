use core::{fmt::{Debug, Display}, ptr};

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct VolatilePtr<T> {
    ptr: *mut T,
}

impl<T> VolatilePtr<T> {
    pub const fn new(ptr: *mut T) -> Self { Self { ptr } }

    pub fn read(&self) -> T
    where T: Copy,
    { unsafe { ptr::read_volatile(self.ptr) } }

    pub fn write(&self, value: T) 
    where T: Copy,
    { unsafe { ptr::write_volatile(self.ptr, value); } }

    pub fn update<F>(&self, f: F)
    where T: Copy,
        for<'a> F: FnOnce(&'a mut T),
    {
        let mut value = self.read();
        f(&mut value);
        self.write(value);
    }

    pub fn add(&mut self, count: usize) -> Self { VolatilePtr { ptr: unsafe { self.ptr.add(count) } } }
    pub fn sub(&mut self, count: usize) -> Self { VolatilePtr { ptr: unsafe { self.ptr.sub(count) } } }

    pub fn as_ptr(&self) -> *mut T { self.ptr }

    pub fn cast<U>(self) -> VolatilePtr<U> { VolatilePtr { ptr: self.ptr.cast() } }

    pub fn update_and_then<F, U>(&self, f: F) -> U
    where T: Copy,
        for<'a> F: FnOnce(&'a mut T) -> U,
    {
        let mut value = self.read();
        let ret = f(&mut value);
        self.write(value);
        ret
    }
}

unsafe impl<T: Send> Send for VolatilePtr<T> {}
unsafe impl<T: Sync> Sync for VolatilePtr<T> {}

impl<T> Debug for VolatilePtr<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("VolatilePtr")
            .field(&self.ptr)
            .finish()
    }
}

impl<T> Display for VolatilePtr<T>
where T: Display + Copy,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let value = self.read();
        writeln!(f, "{}", value)
    }
}
