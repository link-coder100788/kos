use core::{arch::asm, pin::Pin};
use alloc::{boxed::Box, sync::Arc, task::Wake, vec::Vec};
use crate::std::lock::mutex::Mutex;

pub struct AsyncTask {
    pub future: Mutex<Option<Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>>>,
    pub task_id: usize,
}

pub static ASYNC_TASK_QUEUE: Mutex<Vec<Arc<AsyncTask>>> = Mutex::new(Vec::new());

impl Wake for AsyncTask {
    fn wake(self: Arc<Self>) {
        ASYNC_TASK_QUEUE.lock().push(self.clone());
        crate::arch::smp::notify_async_work();
    }
}

pub fn spawn<F>(future: F)
where F: Future<Output = ()> + Send + Sync + 'static,
{
    static NEXT_ID: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let task_id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let task = Arc::new(AsyncTask {
        future: Mutex::new(Some(Box::pin(future))),
        task_id,
    });
    ASYNC_TASK_QUEUE.lock().push(task);
    crate::arch::smp::notify_async_work();
}

pub mod test {
    use crate::id_println;
    use core::sync::atomic::AtomicBool;
    use core::sync::atomic::Ordering;

    static TEST_COMPLETED: AtomicBool = AtomicBool::new(false);
    static TEST_PASSED: AtomicBool = AtomicBool::new(false);

    pub async fn add_async(a: i32, b: i32) -> i32 {
        crate::arch::timer::delay_ms(2000);
        id_println!("finished");
        a + b
    }

    pub fn run_async_test() {
        id_println!("[TEST] Spawning async_add test");
        super::spawn(async {
            let result = add_async(5, 10).await;
            if result == 15 {
                TEST_PASSED.store(true, Ordering::SeqCst);
            }
            TEST_COMPLETED.store(true, Ordering::SeqCst);
        });
        while !TEST_COMPLETED.load(Ordering::SeqCst) {
            core::hint::spin_loop();
        }
        if TEST_PASSED.load(Ordering::SeqCst) {
            id_println!("[Test] add_async: PASSED");
        } else {
            id_println!("[Test] add_async: FAILED");
        }
    }
}