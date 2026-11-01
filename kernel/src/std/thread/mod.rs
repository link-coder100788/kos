use core::{arch::asm, marker::PhantomData, sync::atomic::AtomicUsize, time::Duration};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use crate::std::{lock::mutex::Mutex, refs::refcell::RefCell};
use core::sync::atomic::Ordering;

pub type TaskFn = Box<dyn FnOnce() + Send + 'static>;

pub struct Task {
    pub run: TaskFn,
    pub counter: Option<Arc<AtomicUsize>>,
}

pub static TASK_QUEUE: Mutex<RefCell<Vec<Task>>> = Mutex::new(RefCell::new(Vec::new()));

pub fn spawn<F>(f: F)
where F: FnOnce() + Send + 'static,
{
    let guard = TASK_QUEUE.lock();
    guard.borrow_mut().push(Task { run: Box::new(f), counter: None });
    drop(guard);
    crate::arch::smp::notify_thread_work();
}

pub fn sleep(dur: Duration) {
    crate::arch::timer::delay_ms(dur.as_millis() as u64);
}

pub fn current() -> u64 {
    let core: u64;
    unsafe { asm!("mrs {0}, mpidr_el1", out(reg) core) }
    (core & 0xff)
}

pub struct Scope<'a, 'b: 'a> {
    active_tasks: Arc<AtomicUsize>,
    _phantom: PhantomData<&'a &'b ()>,
}

impl<'a, 'b> Scope<'a, 'b> {
    pub fn spawn<F>(&self, f: F)
    where F: FnOnce() + Send + 'a,
    {
        self.active_tasks.fetch_add(1, Ordering::SeqCst);
        let active_tasks = Arc::clone(&self.active_tasks);
        let wrapped_task = move || {
            f();
            active_tasks.fetch_sub(1, Ordering::SeqCst);
            unsafe { core::arch::asm!("sev"); }
        };
        let erased_task: Box<dyn FnOnce() + Send + 'static> = unsafe {
            core::mem::transmute::<
                Box<dyn FnOnce() + Send + 'a>,
                Box<dyn FnOnce() + Send + 'static>,
            >(Box::new(wrapped_task))
        };
        let guard = TASK_QUEUE.lock();
        guard.borrow_mut().push(Task {
            run: erased_task,
            counter: None,
        });
        drop(guard);
        crate::arch::smp::notify_thread_work();
    }
}

pub fn scope<'b, F, T>(f: F) -> T
where F: for<'a> FnOnce(&'a Scope<'a, 'b>) -> T,
{
    let active_tasks = Arc::new(AtomicUsize::new(0));
    let scope = Scope {
        active_tasks: Arc::clone(&active_tasks),
        _phantom: core::marker::PhantomData,
    };
    let result = f(&scope);
    while active_tasks.load(Ordering::SeqCst) > 0 {
        crate::scheduler::relax();
    }
    result
}