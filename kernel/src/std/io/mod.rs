pub mod fs;
pub mod bytes;

pub enum IoError {
    Generic,
}

// ---- std::io-style errors, Read and Write ----------------------------------------
// The parts of std::io that sockets (std::net) need, with the same names and
// meanings, so code written against std reads the same here.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    ConnectionRefused,
    ConnectionReset,
    NotConnected,
    AddrInUse,
    InvalidInput,
    WouldBlock,
    TimedOut,
    UnexpectedEof,
    WriteZero,
    Other,
}

#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, message)
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind, alloc::format!("{:?}", kind))
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type Result<T> = core::result::Result<T, Error>;

pub trait Read {
    /// Reads some bytes into `buf`; 0 means the end (or an empty `buf`).
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.read(buf)? {
                0 => return Err(Error::new(ErrorKind::UnexpectedEof, "failed to fill whole buffer")),
                n => buf = &mut buf[n..],
            }
        }
        Ok(())
    }

    /// Reads until the end, appending to `buf`. Returns how much was read.
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let start = buf.len();
        let mut chunk = [0u8; 2048];
        loop {
            match self.read(&mut chunk)? {
                0 => return Ok(buf.len() - start),
                n => buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        let mut bytes = Vec::new();
        let n = self.read_to_end(&mut bytes)?;
        let text = core::str::from_utf8(&bytes).map_err(|_| Error::new(ErrorKind::InvalidInput, "stream did not contain valid UTF-8"))?;
        buf.push_str(text);
        Ok(n)
    }
}

pub trait Write {
    /// Writes some of `buf`, returning how much.
    fn write(&mut self, buf: &[u8]) -> Result<usize>;

    fn flush(&mut self) -> Result<()>;

    fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
        while !buf.is_empty() {
            match self.write(buf)? {
                0 => return Err(Error::new(ErrorKind::WriteZero, "failed to write whole buffer")),
                n => buf = &buf[n..],
            }
        }
        Ok(())
    }

    fn write_fmt(&mut self, args: core::fmt::Arguments) -> Result<()> {
        self.write_all(alloc::format!("{}", args).as_bytes())
    }
}
