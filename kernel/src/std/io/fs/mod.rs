pub mod read;
pub mod write;
pub mod file;

pub enum FileError {
    NotFound,
}

pub enum FsResult<T> {
    Ok(T),
    Err(FileError),
}