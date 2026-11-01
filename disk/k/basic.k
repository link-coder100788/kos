module math_test;

fn add(a: u64, b: u64) -> u64 {
    return a + b;
}

fn main() {
    let x: u64 = 10;
    mut y: u64 = 20;
    y = y + x;

    let sum: u64 = add(x, y);

    mut i: u64 = 0;
    while i < 5 {
        i++;
    }

    loop {
        if i > 3 {
            break;
        }
        i++;
    }

    let arr: u64[4] = [1,2,3,4];
    mut total: u64 = 0;
    mut j: u64 = 0;
    while j < 4 {
        total = total + arr[j];
        j++;
    }

    let ptrtest: u64 pointer = &x;
}
