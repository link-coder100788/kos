// Checks tcclibc's way into the kernel: one call through kos_syscall for
// each syscall group (except reading a line, windows and dump, which need a
// person or print a lot), and fp.S's one-instruction maths. Prints a PASS
// or FAIL line for each, then how many failed.
#include "../tcclibc/sys.h"
#include "../tcclibc/syscall.h"
#include "../tcclibc/stdio.h"
#include "../tcclibc/stdlib.h"
#include "../tcclibc/string.h"
#include "../tcclibc/math.h"
#include "../tcclibc/crypto.h"
#include "../tcclibc/net.h"

static int failed;

static void check(const char* what, int ok) {
    printf("%s %s\n", ok ? "PASS" : "FAIL", what);
    if (!ok) failed++;
}

void _start(void) {
    // 0x1, 0x2: writing.
    uint8_t bang = '!';
    write_byte(&bang);
    write_bytes((const uint8_t*)"\n", 1);
    check("write_byte, write_bytes (a ! above)", 1);

    // 0x4: memory.
    size_t free_before = mem_free();
    char* p = alloc_z(100000);
    check("alloc_z gives zeroed memory", p && p[0] == 0 && p[99999] == 0);
    check("mem_size", p && mem_size(p) >= 100000);
    free(p);
    check("mem_free, mem_total", free_before > 0 && mem_total() >= free_before);

    // 0x5: files.
    const char* text = "written by tcc";
    char buf[64] = {0};
    check("fs_write", fs_write("/tcc-test.txt", text, strlen(text)) >= 0);
    check("fs_size", fs_size("/tcc-test.txt") == (long)strlen(text));
    check("fs_read", fs_read("/tcc-test.txt", buf, sizeof(buf) - 1) == (long)strlen(text) && strcmp(buf, text) == 0);
    check("fs_remove", fs_remove("/tcc-test.txt") >= 0 && fs_size("/tcc-test.txt") < 0);

    // 0x7: time.
    uint64_t t0 = uptime_ms();
    sleep_ms(50);
    uint64_t slept = uptime_ms() - t0;
    check("sleep_ms, uptime_ms", slept >= 50 && slept < 1000);
    check("unix_time is after 2024", unix_time() > 1704067200L);

    // 0x9, 0xa: yield, arguments (none when run without any).
    yield();
    check("yield, args", args(NULL, 0) == 0);

    // 0xb: random numbers.
    check("random_u64", random_u64() != random_u64());

    // 0xc: the network (only asks for KOS's own address).
    char addr[32] = {0};
    check("net_address", net_address(addr, sizeof(addr)) > 0 && strchr(addr, '.') != NULL);

    // 0xd: processes.
    long me = proc_self();
    check("proc_self, proc_running", me > 0 && proc_running(me));

    // 0xe: uname.
    utsname_t u;
    check("uname", uname(&u) == 0 && strcmp(u.sysname, "KOS") == 0);

    // 0xf: crypto. SHA-256 of "abc" starts ba 78 16 bf.
    uint8_t digest[SHA256_LEN];
    sha256("abc", 3, digest);
    check("sha256", digest[0] == 0xba && digest[1] == 0x78 && digest[2] == 0x16 && digest[3] == 0xbf);

    // 0x10: sound.
    check("snd_busy", snd_busy() == 0);

    // A syscall number kos_syscall doesn't have.
    check("unknown syscall gives -1", (long)KOS_SVC(99, 0, 0, 0, 0, 0, 0) == -1);

    // fp.S, and the INFINITY / NAN / sign bit replacements in stdio and stdlib.
    check("sqrt", sqrt(2.0) > 1.41421 && sqrt(2.0) < 1.41422);
    check("floor, ceil", floor(-2.5) == -3.0 && ceil(-2.5) == -2.0);
    check("round, trunc", round(2.5) == 3.0 && round(-2.5) == -3.0 && trunc(-2.7) == -2.0);
    snprintf(buf, sizeof(buf), "%.3f %f %f %.1f", 3.14159, INFINITY, NAN, -0.0);
    check("printf of doubles", strcmp(buf, "3.142 inf nan -0.0") == 0);
    check("strtod", strtod("1e500", NULL) == INFINITY && isnan(strtod("nan", NULL)) && strtod("2.5", NULL) == 2.5);

    printf("%d failed\n", failed);
    exit();
}
