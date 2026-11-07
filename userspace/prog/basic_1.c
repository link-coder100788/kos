// Sums 0..9 and returns instead of calling exit(0), to check that a program
// returning from _start is cleaned up by the kernel.
int _start(void) {
    int sum = 0;
    for (int i = 0; i < 10; i++) {
        sum += i;
    }
    return sum;
}
