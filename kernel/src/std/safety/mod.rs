pub struct UserMemoryManager;

impl UserMemoryManager {
    pub const USER_START: usize = 0x5000_0000;
    pub const USER_END:   usize = 0x6000_0000;
    
    pub fn validate_range(&self, ptr: *const u8, len: usize) -> Result<(), &'static str> {
        let start = ptr as usize;
        let end = start.checked_add(len).ok_or("Security Violation: Pointer range overflows!")?;
        if start < Self::USER_START || end > Self::USER_END {
            return Err("Security Violation: Out of bounds user-space memory access!");
        }
        Ok(())
    }

    pub fn with_user_slice<F, T>(&self, user_ptr: *const u8, len: usize, mut f: F) -> Result<T, &'static str>
    where
        F: for<'a> FnOnce(&'a [u8]) -> T,
    {
        self.validate_range(user_ptr, len)?;
        let safe_slice = unsafe { core::slice::from_raw_parts(user_ptr, len) };
        Ok(f(safe_slice))
    }

    pub fn with_user_str<F, T>(&self, user_ptr: *const u8, max_len: usize, mut f: F) -> Result<T, &'static str>
    where
        F: for<'a> FnOnce(&'a str) -> T,
    {
        let start = user_ptr as usize;
        if start < Self::USER_START || start >= Self::USER_END {
            return Err("Security Violation: String pointer starts outside user-space range!");
        }
        let mut len = 0;
        unsafe {
            while len < max_len {
                let current_addr = start + len;
                if current_addr >= Self::USER_END {
                    return Err("Security Violation: String reached end of user-space without null-terminator!");
                }
                let current_byte = (current_addr as *const u8).read_volatile();
                if current_byte == b'\0' {
                    break;
                }
                len += 1;
            }
        }
        if len == max_len {
            return Err("Security Violation: String exceeded maximum allowed length!");
        }
        let raw_slice = unsafe { core::slice::from_raw_parts(user_ptr, len) };
        let safe_str = core::str::from_utf8(raw_slice)
            .map_err(|_| "Security Violation: String is not valid UTF-8")?;
        Ok(f(safe_str))
    }
}