use alloc::{boxed::Box, collections::{btree_map::BTreeMap, vec_deque::VecDeque}};
use crate::std::lock::mutex::Mutex;
use lazy_static::lazy_static;

#[derive(Debug)]
pub enum MessageData {
    Small([u8; 16]),
    Large(Box<[u8]>),
    Signal(u32),
}

#[derive(Debug)]
pub struct Message {
    pub sender_id: u32,
    pub payload: MessageData,
}

pub struct Mailbox {
    queue: Mutex<VecDeque<Message>>,
}

impl Mailbox {
    pub const fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn send(&self, message: Message) {
        let mut guard = self.queue.lock();
        guard.push_back(message);
    }

    /// Non-Blocking
    pub fn try_recv(&self) -> Option<Message> {
        let mut guard = self.queue.lock();
        guard.pop_front()
    }
}

lazy_static! {
    pub static ref IPC_REGISTRY: Mutex<BTreeMap<u32, &'static Mailbox>> = Mutex::new(BTreeMap::new());
}

pub fn register_mailbox(id: u32, mailbox: &'static Mailbox) {
    IPC_REGISTRY.lock().insert(id, mailbox);
}

pub fn send_message(target_task_id: u32, sender_id: u32, payload: MessageData) -> Result<(), &'static str> {
    let registry = IPC_REGISTRY.lock();
    if let Some(mailbox) = registry.get(&target_task_id) {
        mailbox.send(Message { sender_id, payload });
        Ok(())
    } else {
        Err("Target mailbox not found")
    }
}