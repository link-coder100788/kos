use alloc::collections::VecDeque;
use alloc::sync::Arc;
use crate::std::lock::mutex::Mutex;

struct ChannelInner<T> {
    queue: VecDeque<T>,
    senders: usize,
}

pub struct Sender<T> {
    inner: Arc<Mutex<ChannelInner<T>>>,
}

pub struct Receiver<T> {
    inner: Arc<Mutex<ChannelInner<T>>>,
}

pub enum TryRecvError {
    Empty,
    Disconnected,
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Mutex::new(ChannelInner { queue: VecDeque::new(), senders: 1 }));
    (Sender { inner: Arc::clone(&inner) }, Receiver { inner })
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) {
        let mut inner = self.inner.lock();
        inner.queue.push_back(value);
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let mut inner = self.inner.lock();
        inner.senders += 1;
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        inner.senders -= 1;
    }
}

impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut inner = self.inner.lock();
        if let Some(data) = inner.queue.pop_front() {
            Ok(data)
        } else if inner.senders == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
}