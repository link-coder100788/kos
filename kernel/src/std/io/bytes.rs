use crate::std::io::IoError;

pub struct Bytes<'a, T: Sized + AsRef<[u8]>> {
    data: &'a T,
    position: usize,
}

impl<'a, T: Sized + AsRef<[u8]>> Iterator for Bytes<'a, T> {
    type Item = Result<u8, IoError>;

    fn next(&mut self) -> Option<Self::Item> {
        let slice = self.data.as_ref();
        if self.position < slice.len() {
            let byte = slice[self.position];
            self.position += 1;
            Some(Ok(byte))
        } else {
            None
        }
    }
}