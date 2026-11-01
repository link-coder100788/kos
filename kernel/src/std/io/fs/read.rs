use crate::std::io::fs::FsResult;
use alloc::vec::Vec;
use alloc::string::String;
use super::super::bytes::Bytes;

pub enum ReadSource<'a> {
    File(&'a str),
    Memory(*const u8, usize),
    Uart,
}

pub trait Read {
    fn read(&self, buf: &mut [u8], from: ReadSource) -> FsResult<usize>;
    fn read_at(&self, buf: &mut [u8], offset: usize, from: ReadSource) -> FsResult<usize>;
    fn read_to_string(&self, from: ReadSource) -> FsResult<String>;
    fn read_to_vec(&self, from: ReadSource) -> FsResult<Vec<u8>>;
    fn bytes<'a>(&'a self) -> Bytes<'a, Self>
    where Self: Sized + AsRef<[u8]>;
    fn read_and_then<'a, F, T>(&'a self, f: F) -> FsResult<T>
    where F: FnOnce(Bytes<'a, Self>), Self: Sized + AsRef<[u8]>;
}

pub trait LendingRead {
    type Error;

    fn read_and_then<F, T>(&mut self, f: F) -> Result<T, Self::Error>
    where for<'a> F: FnOnce(&'a [u8]) -> T;
}