/// What `uname` reports, in the order its fields are laid out.
pub const SYSTEM_UNAME: [&str; 5] = [
    "KOS",              // sysname
    "kos",              // nodename
    "0.1.0",            // release
    "KOS Kernel 0.1.0", // version
    "aarch64",          // machine
];

pub trait Toggle {
    fn toggle(&mut self);
}

impl Toggle for bool {
    fn toggle(&mut self) {
        *self = !*self;
    }
}

pub fn is_prime(n: u64) -> bool {
    if n <= 1 { return false; }
    let mut i = 3;
    let limit = libm::sqrt(n as f64) as u64;
    if n % 2 == 0 { return n == 2; }
    while i <= limit {
        if n % i == 0 { return false; }
        i += 2;
    }
    true
}
