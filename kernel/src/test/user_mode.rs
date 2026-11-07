//! Tests of programs in user mode: each starts a tiny hand-assembled
//! program (wrapped as a K image, which loads at `USER_BASE` as is) and
//! checks how it ends. Programs that fault are counted in
//! `stats::PROGRAMS_KILLED`; ones ended with `kill` aren't.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::loader;
use crate::process::{self, Console, USER_BASE};
use crate::scheduler::{self, wait::sleep_ms, TaskId};
use crate::stats::PROGRAMS_KILLED;

// ---- a few AArch64 instructions -------------------------------------------------

const fn svc(n: u32) -> u32 { 0xd400_0001 | n << 5 }
const fn movz(rd: u32, imm: u32, shift: u32) -> u32 { 0xd280_0000 | (shift / 16) << 21 | imm << 5 | rd }
const fn movk(rd: u32, imm: u32, shift: u32) -> u32 { 0xf280_0000 | (shift / 16) << 21 | imm << 5 | rd }

/// `x{rd} = value`, in four instructions.
fn mov64(rd: u32, value: u64) -> [u32; 4] {
    let part = |i: u32| ((value >> (16 * i)) & 0xffff) as u32;
    [movz(rd, part(0), 0), movk(rd, part(1), 16), movk(rd, part(2), 32), movk(rd, part(3), 48)]
}

const EXIT: u32 = svc(0);
const LOOP: u32 = 0x1400_0000;          // b .
const RET: u32 = 0xd65f_03c0;           // ret
const BRK: u32 = 0xd420_0000;           // brk #0: faults, so the program is killed
const LDR_X0_X1: u32 = 0xf940_0020;     // ldr x0, [x1]
const STR_X0_X1: u32 = 0xf900_0020;     // str x0, [x1]
const STR_XZR_X0: u32 = 0xf900_001f;    // str xzr, [x0]
const CMN_X0_1: u32 = 0xb100_041f;      // cmn x0, #1 (is x0 == u64::MAX?)
const B_EQ_SKIP: u32 = 0x5400_0040;     // b.eq over the next instruction
const CBZ_X0_SKIP: u32 = 0xb400_0040;   // cbz x0, over the next instruction
const CBNZ_X0_SKIP: u32 = 0xb500_0040;  // cbnz x0, over the next instruction
const ADD_X1_X0_X2: u32 = 0x8b02_0001;  // add x1, x0, x2
const STR_XZR_X1: u32 = 0xf900_003f;    // str xzr, [x1]
const MOV_X19_X0: u32 = 0xaa00_03f3;    // mov x19, x0
const MOV_X0_X19: u32 = 0xaa13_03e0;    // mov x0, x19
const MOV_X0_SP: u32 = 0x9100_03e0;     // mov x0, sp
const SUB_SP_1M: u32 = 0xd144_03ff;     // sub sp, sp, #1 MiB
const SUBS_X2_1: u32 = 0xf100_0442;     // subs x2, x2, #1
const B_NE_BACK_3: u32 = 0x54ff_ffa1;   // b.ne back three instructions
const B_NE_SKIP: u32 = 0x5400_0041;     // b.ne over the next instruction
const MASK_IRQS: u32 = 0xd503_42df;     // msr daifset, #2 (privileged)
const SUB_SP_4K: u32 = 0xd140_07ff;     // sub sp, sp, #4096
const STR_XZR_SP: u32 = 0xf900_03ff;    // str xzr, [sp]
const B_BACK_2: u32 = 0x17ff_fffe;      // b back two instructions

const KERNEL_ADDRESS: u64 = 0xffff_ff80_4020_0000;

/// A K image whose code is `code`, entered at its first instruction.
fn kbin(code: &[u32]) -> Vec<u8> {
    let mut image = Vec::new();
    image.extend_from_slice(b"KBIN");
    image.extend_from_slice(&1u32.to_le_bytes());
    image.extend_from_slice(&(USER_BASE as u64).to_le_bytes());
    image.extend_from_slice(&(code.len() as u64 * 4).to_le_bytes());
    image.extend_from_slice(&0u64.to_le_bytes());
    for word in code {
        image.extend_from_slice(&word.to_le_bytes());
    }
    image
}

fn start(code: &[u32]) -> TaskId {
    loader::start_image("test", "", &kbin(code), true, Console::Detached, None).expect("starting a test program")
}

fn running(id: TaskId) -> bool {
    scheduler::tasks().iter().any(|t| t.id == id)
}

/// Waits up to `ms` for task `id` to end.
fn ended_within(id: TaskId, ms: u64) -> bool {
    for _ in 0..ms / 10 {
        if !running(id) {
            return true;
        }
        sleep_ms(10);
    }
    !running(id)
}

/// Runs `code` to the end; returns whether it was killed for a fault.
fn run_to_end(code: &[u32]) -> bool {
    let killed_before = PROGRAMS_KILLED.load(Ordering::Relaxed);
    let id = start(code);
    assert!(ended_within(id, 2000), "test program didn't end");
    PROGRAMS_KILLED.load(Ordering::Relaxed) != killed_before
}

fn test_program_exits() {
    assert!(!run_to_end(&[movz(0, 0, 0), EXIT]));
}

fn test_returning_from_start_exits() {
    assert!(!run_to_end(&[RET]));
}

fn test_reading_kernel_memory_kills() {
    let mut code = Vec::from(mov64(1, KERNEL_ADDRESS));
    code.extend([LDR_X0_X1, EXIT]);
    assert!(run_to_end(&code));
}

fn test_writing_kernel_memory_kills() {
    let mut code = Vec::from(mov64(1, KERNEL_ADDRESS));
    code.extend([STR_X0_X1, EXIT]);
    assert!(run_to_end(&code));
}

fn test_privileged_instruction_kills() {
    assert!(run_to_end(&[MASK_IRQS, EXIT]));
}

fn test_stack_overflow_hits_guard_page() {
    assert!(run_to_end(&[SUB_SP_4K, STR_XZR_SP, B_BACK_2]));
}

fn test_syscall_with_kernel_pointer_fails() {
    // read_line into kernel memory must return u64::MAX, not write there.
    let mut code = Vec::from(mov64(0, KERNEL_ADDRESS));
    code.extend([movz(1, 64, 0), svc(3), CMN_X0_1, B_EQ_SKIP, BRK, EXIT]);
    assert!(!run_to_end(&code));
    // write_bytes from kernel memory: ignored, and the kernel carries on.
    let mut code = Vec::from(mov64(0, KERNEL_ADDRESS));
    code.extend([movz(1, 64, 0), svc(2), EXIT]);
    assert!(!run_to_end(&code));
}

fn test_heap_memory_is_the_programs_own() {
    // alloc(100), write to it, free it; free must return 0.
    let code = [
        movz(0, 100, 0), movz(16, 0, 0), svc(4),
        STR_XZR_X0,
        movz(16, 6, 0), svc(4),
        CBZ_X0_SKIP, BRK, EXIT,
    ];
    assert!(!run_to_end(&code));
}

fn test_programs_have_separate_memory() {
    // One program writes to its memory and keeps running; another reads
    // the same address in its own address space, and must see zero. (It's
    // in the page holding each one's code: only that much is mapped.)
    let address = (USER_BASE + 0x800) as u64;
    let mut writer = Vec::from(mov64(1, address));
    writer.extend([movz(0, 0x1234, 0), STR_X0_X1, LOOP]);
    let writer = start(&writer);
    sleep_ms(50);
    let mut reader = Vec::from(mov64(1, address));
    reader.extend([LDR_X0_X1, CBZ_X0_SKIP, BRK, EXIT]);
    let killed = run_to_end(&reader);
    process::kill(writer).unwrap();
    assert!(ended_within(writer, 1000));
    assert!(!killed, "a program saw another's memory");
}

fn test_big_allocation_grows_the_heap() {
    // alloc(64 MiB), more than a program used to get in all, then write
    // its first and last words.
    let mut code = Vec::from(mov64(0, 64 << 20));
    code.extend([movz(16, 0, 0), svc(4), CBNZ_X0_SKIP, BRK]);
    code.extend(mov64(2, (64 << 20) - 8));
    code.extend([ADD_X1_X0_X2, STR_XZR_X0, STR_XZR_X1, EXIT]);
    assert!(!run_to_end(&code));
}

fn test_big_program_image_loads() {
    // A 24 MiB image: an exit, then zeroes.
    let mut code = alloc::vec![0u32; 6 << 20];
    code[0] = EXIT;
    assert!(!run_to_end(&code));
}

fn test_many_programs_at_once() {
    // Well past the 16 slots programs used to be limited to.
    let ids: Vec<TaskId> = (0..40).map(|_| start(&[LOOP])).collect();
    sleep_ms(100);
    assert!(ids.iter().all(|&id| running(id)));
    assert!(process::list().len() >= 40);
    for &id in &ids {
        process::kill(id).unwrap();
    }
    assert!(ids.iter().all(|&id| ended_within(id, 2000)));
}

fn test_finished_programs_give_memory_back() {
    // Give the loader task a moment to reap what earlier tests left.
    // Frames the kernel heap grows into meanwhile don't count.
    let free = || crate::mm::frame::stats().0 * crate::mm::layout::PAGE_SIZE + crate::mm::allocator::ALLOCATOR.heap_size().0;
    sleep_ms(200);
    let before = free();
    let mut code = Vec::from(mov64(0, 8 << 20));
    code.extend([movz(16, 0, 0), svc(4), EXIT]);
    assert!(!run_to_end(&code));
    sleep_ms(200);
    assert_eq!(free(), before, "a program's pages weren't freed");
}

fn memory_of(id: TaskId) -> usize {
    process::list().iter().find(|p| p.task == id).map_or(0, |p| p.memory)
}

fn test_freed_heap_pages_go_back() {
    // alloc(8 MiB), wait, free it, then loop.
    let mut code = Vec::from(mov64(0, 8 << 20));
    code.extend([movz(16, 0, 0), svc(4), MOV_X19_X0]);
    code.extend([movz(0, 300, 0), movz(16, 0, 0), svc(7)]);
    code.extend([MOV_X0_X19, movz(16, 6, 0), svc(4), LOOP]);
    let id = start(&code);
    sleep_ms(150);
    let holding = memory_of(id);
    sleep_ms(400);
    let after = memory_of(id);
    process::kill(id).unwrap();
    assert!(ended_within(id, 1000));
    assert!(holding >= 8 << 20, "the allocation wasn't mapped: {} bytes", holding);
    assert!(after < 1 << 20, "freeing didn't give the pages back: {} bytes", after);
}

fn test_stack_grows_on_demand() {
    // Use 4 MiB of stack, a page at a time, then exit: it starts with
    // 16 KiB, and must grow rather than be killed.
    let code = [movz(2, 1024, 0), SUB_SP_4K, STR_XZR_SP, SUBS_X2_1, B_NE_BACK_3, EXIT];
    assert!(!run_to_end(&code));
}

fn test_syscall_buffer_on_untouched_stack() {
    // read_line into a buffer 1 MiB down the stack, never touched: the
    // kernel must grow the stack, not call the pointer bad. (It's given a
    // line to read: at the end of its input it would get -1 too.)
    let code = [SUB_SP_1M, MOV_X0_SP, movz(1, 64, 0), svc(3), CMN_X0_1, B_NE_SKIP, BRK, EXIT];
    let (r, w) = crate::fd::pipe();
    w.write(b"line\n", true).unwrap();
    let killed_before = PROGRAMS_KILLED.load(Ordering::Relaxed);
    let id = start_with(&code, [r, crate::fd::null(), crate::fd::null()]);
    assert!(ended_within(id, 2000), "test program didn't end");
    assert_eq!(PROGRAMS_KILLED.load(Ordering::Relaxed), killed_before);
}

fn test_kill_ends_a_looping_program() {
    let id = start(&[LOOP]);
    sleep_ms(50);
    assert!(running(id));
    process::kill(id).unwrap();
    assert!(ended_within(id, 1000));
}

fn test_killed_programs_sound_stops() {
    // snd(440) (svc 16, sub-command 0), then loop: its tone plays until
    // it's killed, and stops once it's been reaped.
    let id = start(&[movz(0, 440, 0), movz(16, 0, 0), svc(16), LOOP]);
    let sounding_within = |ms: u64, want: bool| (0..ms / 10).any(|_| { sleep_ms(10); crate::sound::busy() == want });
    assert!(sounding_within(1000, true), "the program's tone never started");
    process::kill(id).unwrap();
    assert!(ended_within(id, 1000));
    assert!(sounding_within(1000, false), "the tone outlived its program");
}

fn test_kill_wakes_a_sleeping_program() {
    // sleep_ms(60000), then exit: killing it mustn't wait a minute.
    let id = start(&[movz(0, 60000, 0), movz(16, 0, 0), svc(7), EXIT]);
    sleep_ms(50);
    process::kill(id).unwrap();
    assert!(ended_within(id, 1000));
}

fn program_cores() -> Vec<usize> {
    scheduler::PROGRAM_CORES.iter().copied().filter(|&c| scheduler::is_running(c)).collect()
}

fn core_of(id: TaskId) -> usize {
    scheduler::tasks().iter().find(|t| t.id == id).unwrap().core
}

fn test_programs_run_on_program_cores() {
    let cores = program_cores();
    let ids: Vec<TaskId> = (0..4).map(|_| start(&[LOOP])).collect();
    sleep_ms(50);
    let used: Vec<usize> = ids.iter().map(|&id| core_of(id)).collect();
    for &id in &ids {
        process::kill(id).unwrap();
        assert!(ended_within(id, 1000));
    }
    if cores.is_empty() {
        assert!(used.iter().all(|&c| c == 0), "no program cores: programs belong on core 0");
    } else {
        assert!(used.iter().all(|c| cores.contains(c)), "a program ran outside the program cores: {:?}", used);
        // Spread out, not all piled onto one core.
        assert!(cores.iter().all(|c| used.contains(c)), "programs not spread over {:?}: {:?}", cores, used);
    }
}

fn test_programs_run_in_parallel() {
    // Two busy programs, on two cores, each get (nearly) a whole core.
    if program_cores().len() < 2 {
        return;
    }
    let ids = [start(&[LOOP]), start(&[LOOP])];
    let ticks = |id: TaskId| scheduler::tasks().iter().find(|t| t.id == id).unwrap().ticks;
    sleep_ms(50);
    let before = ids.map(ticks);
    sleep_ms(500);
    let after = ids.map(ticks);
    for &id in &ids {
        process::kill(id).unwrap();
        assert!(ended_within(id, 1000));
    }
    // 500 ms is 50 ticks; sharing one core would give each about 25.
    for i in 0..2 {
        assert!(after[i] - before[i] >= 40, "program {} only ran {} of 50 ticks", i, after[i] - before[i]);
    }
}

fn test_wakeups_reach_program_cores_quickly() {
    // sleep_ms(10) five times, then exit: each sleep is woken by its own
    // core's timer, but the exit is noticed through a kick from that core
    // to this one. The whole thing should take about 50 ms, not more than
    // double that.
    let mut code = Vec::new();
    for _ in 0..5 {
        code.extend([movz(0, 10, 0), movz(16, 0, 0), svc(7)]);
    }
    code.push(EXIT);
    let started = crate::arch::timer::uptime_ms();
    assert!(!run_to_end(&code));
    let took = crate::arch::timer::uptime_ms() - started;
    assert!(took < 150, "five 10 ms sleeps took {} ms", took);
}

// ---- exit statuses and file descriptors -------------------------------------------

/// Waits up to `ms` for program `id` to end and be reaped, and returns its
/// exit status.
fn status_within(id: TaskId, ms: u64) -> i32 {
    for _ in 0..ms / 10 {
        if let Some(status) = process::exit_status(id) {
            return status.expect("an ended program has a status");
        }
        sleep_ms(10);
    }
    panic!("program {} didn't end", id);
}

/// Starts `code` with `stdio` as its stdin, stdout and stderr.
fn start_with(code: &[u32], stdio: [crate::fd::Fd; 3]) -> TaskId {
    loader::start_image("test", "", &kbin(code), true, Console::Detached, Some(stdio)).expect("starting a test program")
}

fn test_exit_status() {
    assert_eq!(status_within(start(&[movz(0, 42, 0), EXIT]), 2000), 42);
    assert_eq!(status_within(start(&[movz(0, 0x1ff, 0), EXIT]), 2000), 0xff, "kept to 0-255");
    assert_eq!(status_within(start(&[movz(0, 7, 0), RET]), 2000), 0, "returning from _start is exit(0)");
    assert_eq!(status_within(start(&[BRK]), 2000), process::STATUS_FAULTED);
    let looping = start(&[LOOP]);
    sleep_ms(20);
    process::kill(looping).unwrap();
    assert_eq!(status_within(looping, 2000), process::STATUS_KILLED);
}

fn test_stdout_goes_into_a_pipe() {
    // write_bytes("hi there", 8), then exit; the text sits after the code.
    let text = b"hi there";
    let mut code = Vec::from(mov64(0, 0));
    code.extend([movz(1, text.len() as u32, 0), svc(2), movz(0, 0, 0), EXIT]);
    let data_at = USER_BASE as u64 + code.len() as u64 * 4;
    code[..4].copy_from_slice(&mov64(0, data_at));
    code.extend(text.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())));
    let (r, w) = crate::fd::pipe();
    let id = start_with(&code, [crate::fd::null(), w, crate::fd::null()]);
    assert_eq!(status_within(id, 2000), 0);
    let mut buf = [0u8; 32];
    assert_eq!(r.read(&mut buf, true), Ok(Some(text.len())));
    assert_eq!(&buf[..text.len()], text);
    assert_eq!(r.read(&mut buf, true), Ok(Some(0)), "the program's end closed the pipe's write end");
}

fn test_read_line_from_a_pipe() {
    // readln(sp - 64, 64), then exit with its result as the status.
    let code = [0xd101_03ff /* sub sp, sp, #64 */, MOV_X0_SP, movz(1, 64, 0), svc(3), EXIT];
    let (r, w) = crate::fd::pipe();
    w.write(b"hello\nnext\n", true).unwrap();
    drop(w);
    assert_eq!(status_within(start_with(&code, [r, crate::fd::null(), crate::fd::null()]), 2000), 5, "the first line, without its newline");
    let (r, w) = crate::fd::pipe();
    drop(w);
    assert_eq!(status_within(start_with(&code, [r, crate::fd::null(), crate::fd::null()]), 2000), 0xff, "-1 at the end of the input");
}

fn test_kernel_tasks_cant_be_killed() {
    let shell = scheduler::tasks().iter().find(|t| t.name == "shell").map(|t| t.id).unwrap();
    assert!(process::kill(shell).is_err());
    assert!(running(shell));
}

// ---- tcc --------------------------------------------------------------------

fn write_file(path: &str, data: &str) {
    let written = fs_call(FsRequest::Write { cwd: "/".into(), path: path.into(), data: data.as_bytes().to_vec() });
    assert!(matches!(written, FsResponse::Ok), "writing {}", path);
}

fn read_file(path: &str) -> Option<Vec<u8>> {
    match fs_call(FsRequest::Read { cwd: "/".into(), path: path.into() }) {
        FsResponse::Data(data) => Some(data),
        _ => None,
    }
}

fn remove_file(path: &str) {
    fs_call(FsRequest::Remove { cwd: "/".into(), path: path.into() });
}

/// Runs program `path` with `args`, detached, and waits up to `ms` for it.
/// Returns its exit status, and what it printed (stdout and stderr).
fn run_program(path: &str, args: &str, ms: u64) -> (i32, String) {
    let (r, w) = crate::fd::pipe();
    let stdio = [crate::fd::null(), w.clone(), w];
    let id = loader::spawn(path, args, Console::Detached, Some(stdio)).unwrap_or_else(|e| panic!("can't run {}: {}", path, e));
    let status = status_within(id, ms);
    let mut printed = Vec::new();
    let mut buf = [0u8; 512];
    while let Ok(Some(n @ 1..)) = r.read(&mut buf, false) {
        printed.extend_from_slice(&buf[..n]);
    }
    (status, String::from_utf8_lossy(&printed).into_owned())
}

/// /bin/tcc (from `make disk`) compiles a C program on KOS, and what it
/// makes runs. A detached program's output goes nowhere, so the program
/// writes its answer to a file. A program that doesn't compile gives no
/// output file, and tcc fails.
fn test_tcc_compiles_and_runs_a_program() {
    const TCC: &str = "/bin/tcc.elf.b64";
    const SOURCE: &str = "/tcc-test.c";
    const PROGRAM: &str = "/tcc-test.elf";
    const RESULT: &str = "/tcc-test.txt";
    assert!(read_file(TCC).is_some(), "no {} on the disk: run `make disk`", TCC);
    for path in [SOURCE, PROGRAM, RESULT] {
        remove_file(path);
    }
    write_file(SOURCE, r#"
#include <stdio.h>
#include <string.h>

static int square(int x) { return x * x; }

int main(int argc, char** argv) {
    FILE* f = fopen("/tcc-test.txt", "w");
    fprintf(f, "%d %s %d", square(argc + 5), argv[1], (int)strlen(argv[2]));
    fclose(f);
    return 0;
}
"#);
    let (status, printed) = run_program(TCC, &alloc::format!("-o {} {}", PROGRAM, SOURCE), 30_000);
    assert_eq!(status, 0, "tcc failed: {}", printed);
    assert!(read_file(PROGRAM).is_some(), "tcc made no program");
    let (status, printed) = run_program(PROGRAM, "hello abc", 5_000);
    assert_eq!(status, 0, "the program failed: {}", printed);
    let result = read_file(RESULT).map(|d| String::from_utf8_lossy(&d).into_owned());
    assert_eq!(result.as_deref(), Some("64 hello 3"));

    write_file(SOURCE, "int main(void) { return nothing_called_this; }\n");
    remove_file(PROGRAM);
    let (status, printed) = run_program(TCC, &alloc::format!("-o {} {}", PROGRAM, SOURCE), 30_000);
    assert_ne!(status, 0, "tcc succeeded on code with an error");
    assert!(printed.contains("nothing_called_this"), "tcc didn't say what was wrong: {}", printed);
    assert!(read_file(PROGRAM).is_none(), "tcc made a program from code with an error");
    for path in [SOURCE, PROGRAM, RESULT] {
        remove_file(path);
    }
}

pub fn run() {
    // The loader task reaps finished programs; let boot settle first.
    sleep_ms(200);
    crate::test::run_all(&[
        &test_program_exits,
        &test_returning_from_start_exits,
        &test_reading_kernel_memory_kills,
        &test_writing_kernel_memory_kills,
        &test_privileged_instruction_kills,
        &test_stack_overflow_hits_guard_page,
        &test_syscall_with_kernel_pointer_fails,
        &test_heap_memory_is_the_programs_own,
        &test_programs_have_separate_memory,
        &test_big_allocation_grows_the_heap,
        &test_big_program_image_loads,
        &test_many_programs_at_once,
        &test_finished_programs_give_memory_back,
        &test_freed_heap_pages_go_back,
        &test_stack_grows_on_demand,
        &test_syscall_buffer_on_untouched_stack,
        &test_kill_ends_a_looping_program,
        &test_kill_wakes_a_sleeping_program,
        &test_killed_programs_sound_stops,
        &test_programs_run_on_program_cores,
        &test_programs_run_in_parallel,
        &test_wakeups_reach_program_cores_quickly,
        &test_kernel_tasks_cant_be_killed,
        &test_exit_status,
        &test_stdout_goes_into_a_pipe,
        &test_read_line_from_a_pipe,
        &test_tcc_compiles_and_runs_a_program,
    ]);
}
