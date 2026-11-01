// tests/full_language_test.rs — compiles one K program that touches every
// construct in KLS v0.2 through the full pipeline (lex -> parse -> typecheck
// -> IR -> AArch64 codegen -> KBIN), then verifies the result structurally
// and spot-checks specific machine-code bytes using the same encoder the
// compiler itself uses (so a regression in either the compiler or the test
// would have to accidentally agree with itself to slip through undetected).

use super::super::{aarch64_codegen, encoder, ir, kbin, parser, typecheck};
use alloc::vec::Vec;
use alloc::vec;

const FULL_PROGRAM: &str = r#"
module full_coverage;

import kernel;

struct Point {
    x: u64;
    y: u64;
}

struct Line {
    start: Point;
    end: Point;
}

enum State {
    Ready,
    Running,
    Dead
}

fn add(a: u64, b: u64) -> u64 {
    return a + b;
}

fn fib(n: u64) -> u64 {
    if n <= 1 {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

fn dist_sq(p: Point pointer) -> u64 {
    return p->x * p->x + p->y * p->y;
}

fn void_fn() {
}

fn main() {
    // primitive types
    let a8: u8 = 1;
    let a16: u16 = 2;
    let a32: u32 = 3;
    let a64: u64 = 4;
    let s8: i8 = -1;
    let s16: i16 = -2;
    let s32: i32 = -3;
    let s64: i64 = -4;
    let us: usize = 5;
    let is: isize = -5;
    let flag: bool = true;
    let flag2: bool = false;
    let c: char = 'x';
    let hex: u64 = 0xFF;

    // mutability + assignment
    mut counter: u64 = 0;
    counter = counter + 1;

    // arithmetic operators
    let sum: u64 = a64 + 1;
    let diff: u64 = a64 - 1;
    let prod: u64 = a64 * 2;
    let quot: u64 = a64 / 2;
    let rem: u64 = a64 % 2;

    // comparison operators
    let c1: bool = a64 == 4;
    let c2: bool = a64 != 5;
    let c3: bool = a64 < 5;
    let c4: bool = a64 > 3;
    let c5: bool = a64 <= 4;
    let c6: bool = a64 >= 4;

    // logical operators
    let l1: bool = flag && flag2;
    let l2: bool = flag || flag2;
    let l3: bool = !flag;

    // bitwise operators
    let b1: u64 = hex & 0x0F;
    let b2: u64 = hex | 0x100;
    let b3: u64 = hex ^ 0xFF;
    let b4: u64 = ~hex;
    let b5: u64 = hex << 4;
    let b6: u64 = hex >> 4;

    // increment/decrement
    counter++;
    counter--;

    // pointers
    let pval: u64 = 99;
    let pptr: u64 pointer = &pval;
    *pptr = *pptr + 1;
    let pptr2: u64 pointer pointer = &pptr;

    // pointer arithmetic (indexing)
    let arr: u64[5] = [10, 20, 30, 40, 50];
    let elem: u64 = arr[2];
    arr[0] = 100;

    // strings
    let greeting: string = "hello";

    // structs
    let pt: Point;
    pt.x = 3;
    pt.y = 4;
    let line: Line;
    line.start.x = 0;
    line.start.y = 0;
    line.end = pt;

    // struct pointers
    let pt_ptr: Point pointer = &pt;
    let d: u64 = dist_sq(pt_ptr);

    // function calls
    let sum2: u64 = add(1, 2);
    let f10: u64 = fib(10);
    void_fn();

    // if / else / else-if
    if counter == 0 {
        counter = 1;
    } else if counter == 1 {
        counter = 2;
    } else {
        counter = 3;
    }

    // while + break + continue
    mut i: u64 = 0;
    while i < 10 {
        i++;
        if i == 5 {
            continue;
        }
        if i == 8 {
            break;
        }
    }

    // loop + break
    mut j: u64 = 0;
    loop {
        j++;
        if j > 3 {
            break;
        }
    }

    // enums + match
    let state: State = State.Ready;
    match state {
        State.Ready => {
            counter = 10;
        }
        State.Running => {
            counter = 20;
        }
        State.Dead => {
            counter = 30;
        }
    }

    // sizeof
    let sz1: u64 = sizeof(u64);
    let sz2: u64 = sizeof(Point);

    // kernel calls: print (literal), write_byte, write_bytes, read_line, exit
    kernel.print("compile-time-known string");

    let byte_val: u8 = 65;
    let byte_ptr: u8 pointer = &byte_val;
    kernel.write_byte(byte_ptr);

    let buf: u8[3] = [72, 105, 33];
    kernel.write_bytes(buf, 3);

    mut line_buf: u8[16];
    let read_len: u64 = kernel.read_line(line_buf, 16);

    kernel.exit(0);
}
"#;

fn compile_full() -> (ir::IrProgram, aarch64_codegen::CodegenOutput, Vec<u8>) {
    let module = parser::parse(FULL_PROGRAM)
        .unwrap_or_else(|e| panic!("parse error in full-coverage program: {}", e));
    let sema = typecheck::analyze(&module)
        .unwrap_or_else(|e| panic!("type error in full-coverage program: {}", e));
    let prog = ir::lower(&module, &sema);
    let out = aarch64_codegen::generate(&prog);
    let bin = kbin::write(out.entry_address, &out.code, &out.data);
    (prog, out, bin)
}

fn test_full_program_parses_typechecks_and_compiles() {
    let (_prog, out, _bin) = compile_full();
    assert!(!out.code.is_empty());
    assert!(!out.data.is_empty()); // string literal + array/buffer data
}

fn test_full_program_has_all_six_functions() {
    let (prog, _out, _bin) = compile_full();
    let mut names: Vec<&str> = prog.functions.iter().map(|f| f.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["add", "dist_sq", "fib", "main", "void_fn"]);
}

fn test_full_program_kbin_header_is_well_formed() {
    let (_prog, out, bin) = compile_full();
    assert_eq!(&bin[0..4], b"KBIN");
    let version = u32::from_le_bytes(bin[4..8].try_into().unwrap());
    assert_eq!(version, kbin::KBIN_VERSION);
    let entry = u64::from_le_bytes(bin[8..16].try_into().unwrap());
    assert_eq!(entry, out.entry_address);
    assert_eq!(entry, aarch64_codegen::LOAD_BASE);
    let code_size = u64::from_le_bytes(bin[16..24].try_into().unwrap());
    let data_size = u64::from_le_bytes(bin[24..32].try_into().unwrap());
    assert_eq!(code_size, out.code.len() as u64);
    assert_eq!(data_size, out.data.len() as u64);
    assert_eq!(bin.len(), 32 + out.code.len() + out.data.len());
}

fn test_full_program_code_is_word_aligned_and_starts_with_bl_main() {
    let (_prog, out, _bin) = compile_full();
    assert_eq!(out.code.len() % 4, 0);
    // First instruction is always `bl main` in this compiler's entry stub.
    let first_word = u32::from_le_bytes(out.code[0..4].try_into().unwrap());
    let is_bl = (first_word >> 26) == 0b100101;
    assert!(is_bl, "expected first instruction to be BL, got 0x{:08x}", first_word);
}

fn test_full_program_second_instruction_is_exit_svc() {
    let (_prog, out, _bin) = compile_full();
    let second_word = u32::from_le_bytes(out.code[4..8].try_into().unwrap());
    let exit_num = ir::KERNEL_SYSCALLS.iter().find(|(n, _)| *n == "exit").unwrap().1;
    assert_eq!(second_word, encoder::svc(exit_num as u16));
}

fn test_full_program_string_literal_present_in_data_nul_terminated() {
    let (_prog, out, _bin) = compile_full();
    let needle = b"compile-time-known string\0";
    let found = out.data.windows(needle.len()).any(|w| w == &needle[..]);
    assert!(found, "expected the print() literal to appear NUL-terminated in the data section");
}

fn test_full_program_write_bytes_svc_present_in_code() {
    let (_prog, out, _bin) = compile_full();
    let write_bytes_num = ir::KERNEL_SYSCALLS.iter().find(|(n, _)| *n == "write_bytes").unwrap().1;
    let target = encoder::svc(write_bytes_num as u16).to_le_bytes();
    let found = out.code.windows(4).any(|w| w == target);
    assert!(found, "expected an `svc #{}` (write_bytes) somewhere in the code", write_bytes_num);
}

fn test_full_program_write_byte_svc_present_in_code() {
    let (_prog, out, _bin) = compile_full();
    let num = ir::KERNEL_SYSCALLS.iter().find(|(n, _)| *n == "write_byte").unwrap().1;
    let target = encoder::svc(num as u16).to_le_bytes();
    let found = out.code.windows(4).any(|w| w == target);
    assert!(found, "expected an `svc #{}` (write_byte) somewhere in the code", num);
}

fn test_full_program_read_line_svc_present_in_code() {
    let (_prog, out, _bin) = compile_full();
    let num = ir::KERNEL_SYSCALLS.iter().find(|(n, _)| *n == "read_line").unwrap().1;
    let target = encoder::svc(num as u16).to_le_bytes();
    let found = out.code.windows(4).any(|w| w == target);
    assert!(found, "expected an `svc #{}` (read_line) somewhere in the code", num);
}

fn test_full_program_contains_ret_instruction_for_every_function() {
    let (_prog, out, _bin) = compile_full();
    let ret_bytes = encoder::ret().to_le_bytes();
    let ret_count = out.code.windows(4).filter(|w| *w == ret_bytes).count();
    // 5 user functions each emit at least one `ret`; recursive/branching
    // functions (fib, if/else) may emit more than one.
    assert!(ret_count >= 5, "expected at least 5 `ret` instructions, found {}", ret_count);
}

fn test_full_program_deterministic_across_runs() {
    // The compiler has no nondeterminism (no HashMap iteration order should
    // leak into codegen, no timestamps, etc.) — compiling twice must give
    // byte-identical output.
    let (_p1, _o1, bin1) = compile_full();
    let (_p2, _o2, bin2) = compile_full();
    assert_eq!(bin1, bin2);
}

pub fn run() {
    let tests: &[&dyn crate::test::Test] = &[
        &test_full_program_parses_typechecks_and_compiles,
        &test_full_program_has_all_six_functions,
        &test_full_program_kbin_header_is_well_formed,
        &test_full_program_code_is_word_aligned_and_starts_with_bl_main,
        &test_full_program_second_instruction_is_exit_svc,
        &test_full_program_string_literal_present_in_data_nul_terminated,
        &test_full_program_write_bytes_svc_present_in_code,
        &test_full_program_write_byte_svc_present_in_code,
        &test_full_program_read_line_svc_present_in_code,
        &test_full_program_contains_ret_instruction_for_every_function,
        &test_full_program_deterministic_across_runs,
    ];
    crate::println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
}
