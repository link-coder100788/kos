module hello;

import kernel;

fn main() {
    kernel.print("Hello, World!");
    let x: u64 = 100;
    let y: u64 pointer = &x;
    *y = 0;
    kernel.dump();
    kernel.exit(0);
}
