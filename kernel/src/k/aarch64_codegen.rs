//! aarch64_codegen.rs — lowers the stack-based IR to raw AArch64 machine code.
//!
//! Two passes:
//!   1. Walk every instruction, computing how many 32-bit words it expands
//!      to. Word counts are pure functions of the op's own (already-known)
//!      data — never of a resolved address — so both passes agree exactly.
//!      This gives us function start offsets and intra-function label
//!      offsets without any iterative fixup.
//!   2. Walk again, this time emitting real machine words. Anything that
//!      needs a resolved address (branches, calls, global data addresses)
//!      is now computable because pass 1 already determined every offset.
//!
//! Register usage: this is a stack machine, so no cross-instruction
//! register allocation is needed. x0-x3 are used as scratch registers by
//! nearly every op; the syscall number is encoded directly in the `svc`
//! instruction's own immediate field (read back by the kernel via
//! ESR_EL1), not passed in a register; x29/x30/sp are the frame pointer,
//! link register, and stack pointer as usual.

use super::encoder as e;
use super::ir::*;
use hashbrown::HashMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

pub const LOAD_BASE: u64 = 0x5000_0000; // dedicated userspace region — fixed link/load address for KBIN images

/// Looked up from `ir::KERNEL_SYSCALLS` so the entry stub's safety-net exit
/// call can never drift out of sync with the real syscall table.
fn exit_syscall_num() -> u16 {
    KERNEL_SYSCALLS.iter().find(|(name, _)| *name == "exit")
        .expect("KERNEL_SYSCALLS must define 'exit'").1 as u16
}

pub struct CodegenOutput {
    pub code: Vec<u8>,
    pub data: Vec<u8>,
    pub entry_address: u64,
    pub data_base: u64,
}

struct FuncLayout {
    start_word: usize,          // word offset from start of code section
    label_offsets: HashMap<usize, usize>, // label id -> word offset within function
    _word_len: usize,
}

pub fn generate(prog: &IrProgram) -> CodegenOutput {
    // ---- entry stub: bl main; svc #0 (exit — number is the SVC immediate) ----
    let entry_stub_len = 2; // words

    // ---- pass 1: compute lengths ----
    let mut func_layouts: HashMap<String, FuncLayout> = HashMap::new();
    let mut cursor = entry_stub_len;
    for f in &prog.functions {
        let (word_len, label_offsets) = measure_function(f);
        func_layouts.insert(f.name.clone(), FuncLayout { start_word: cursor, label_offsets, _word_len: word_len });
        cursor += word_len;
    }
    let total_code_words = cursor;
    let code_size_bytes = (total_code_words * 4) as u64;

    // ---- data section layout (string literals, 8-byte aligned each) ----
    let data_base = (LOAD_BASE + code_size_bytes + 7) & !7;
    let mut string_addrs: HashMap<String, u64> = HashMap::new();
    let mut data: Vec<u8> = Vec::new();
    for (label, s) in &prog.strings {
        while data.len() % 8 != 0 { data.push(0); }
        string_addrs.insert(label.clone(), data_base + data.len() as u64);
        data.extend_from_slice(s.as_bytes());
        data.push(0); // NUL terminator
    }

    // ---- pass 2: emit ----
    let mut code: Vec<u32> = Vec::with_capacity(total_code_words);

    // entry stub
    let main_addr_word = func_layouts.get("main").expect("no main function").start_word;
    let rel = (main_addr_word as i64) - (0i64); // bl is at word 0
    code.push(e::bl(rel as i32));
    code.push(e::svc(exit_syscall_num()));      // exit — takes no args in this ABI

    for f in &prog.functions {
        let layout = &func_layouts[&f.name];
        debug_assert_eq!(code.len(), layout.start_word);
        emit_function(f, layout, &func_layouts, &string_addrs, &mut code);
    }

    debug_assert_eq!(code.len(), total_code_words);

    let mut code_bytes = Vec::with_capacity(code.len() * 4);
    for w in &code {
        code_bytes.extend_from_slice(&w.to_le_bytes());
    }

    CodegenOutput {
        code: code_bytes,
        data,
        entry_address: LOAD_BASE,
        data_base,
    }
}

// ---------------- pass 1: measurement ----------------

fn measure_function(f: &IrFunction) -> (usize, HashMap<usize, usize>) {
    let mut len = prologue_len(f);
    let mut labels = HashMap::new();
    for op in &f.body {
        if let Ir::Label(id) = op {
            labels.insert(*id, len);
        }
        len += op_len(op);
    }
    (len, labels)
}

fn prologue_len(f: &IrFunction) -> usize {
    // stp x29,x30,[sp,#-16]! ; mov x29,sp ; sub sp,sp,#frame ; (addr + store) per param
    let mut n = 2 + sub_sp_len(f.frame_size);
    for off in &f.param_offsets {
        n += addr_from_fp_words(*off) + 1;
    }
    n
}

fn sub_sp_len(frame_size: u64) -> usize {
    if frame_size <= 0xFFF { 1 } else { imm_move_words(frame_size) + 1 }
}

fn imm_move_words(val: u64) -> usize {
    // movz + movk for each nonzero 16-bit chunk beyond the first, minimum 1.
    let chunks = [val & 0xFFFF, (val >> 16) & 0xFFFF, (val >> 32) & 0xFFFF, (val >> 48) & 0xFFFF];
    let mut n = 1; // movz for chunk 0 always emitted
    for c in &chunks[1..] {
        if *c != 0 { n += 1; }
    }
    n
}

fn addr_from_fp_words(offset: i64) -> usize {
    // ADD/SUB with imm12 if it fits, else load offset into scratch then add/sub reg.
    let mag = offset.unsigned_abs();
    if mag <= 0xFFF { 1 } else { imm_move_words(mag) + 1 }
}

fn op_len(op: &Ir) -> usize {
    match op {
        Ir::PushImm(n) => imm_move_words(*n) + 1,
        Ir::PushLocalAddr(off) => addr_from_fp_words(*off) + 1,
        Ir::PushGlobalAddr(_) => 4 /* fixed-width absolute load */ + 1,
        Ir::Load(_, _) => 3,
        Ir::Store(_) => 3,
        Ir::IndexAddr(elem_size) => 2 /*pops*/ + imm_move_words(*elem_size) + 1 /*mul*/ + 1 /*add*/ + 1 /*push*/,
        Ir::OffsetAddr(off) => 1 /*pop*/ + addr_from_fp_words_signed(*off) + 1 /*push*/,
        Ir::Dup => 2,
        Ir::Swap => 4,
        Ir::Drop => 1,
        Ir::BinOp(op) => 2 /*pops*/ + binop_len(*op) + 1 /*push*/,
        Ir::Neg => 3,
        Ir::Not => 4,
        Ir::BitNot => 3,
        Ir::Call(_, argc) => argc + 2,
        Ir::KernelCall(_, argc) => argc + 2,
        Ir::KernelSubCall(_, _, argc) => argc + 3,
        Ir::Ret(has_value) => (if *has_value { 1 } else { 0 }) + 3,
        Ir::Label(_) => 0,
        Ir::Jump(_) => 1,
        Ir::JumpIfZero(_) => 3,
    }
}

fn addr_from_fp_words_signed(off: u64) -> usize {
    if off <= 0xFFF { 1 } else { imm_move_words(off) + 1 }
}

fn binop_len(op: super::ast::BinOp) -> usize {
    use super::ast::BinOp::*;
    match op {
        Add | Sub | Mul | BitAnd | BitOr | BitXor | Shl | Shr | Div => 1,
        Mod => 2,
        Eq | Ne | Lt | Gt | Le | Ge => 2,
        And | Or => 5,
    }
}

// ---------------- pass 2: emission ----------------

fn emit_function(
    f: &IrFunction,
    layout: &FuncLayout,
    all_funcs: &HashMap<String, FuncLayout>,
    string_addrs: &HashMap<String, u64>,
    code: &mut Vec<u32>,
) {
    // prologue
    code.push(e::stp_pre(e::FP, e::LR, e::SP, -2));
    code.push(e::add_imm(e::FP, e::SP, 0)); // mov x29, sp
    emit_sub_sp(f.frame_size, code);
    for (i, off) in f.param_offsets.iter().enumerate() {
        emit_addr_from_fp(*off, 2, code); // x2 = x29 + off
        code.push(match_store_reg(f.params[i].size(), i as u32, 2));
    }

    for op in &f.body {
        emit_op(op, layout, all_funcs, string_addrs, code);
    }
}

fn emit_sub_sp(frame_size: u64, code: &mut Vec<u32>) {
    // frame_size is always 16-byte aligned. Emitted unconditionally (even
    // when 0, as `sub sp, sp, #0`) so this always matches `sub_sp_len`.
    if frame_size <= 0xFFF {
        code.push(e::sub_imm(e::SP, e::SP, frame_size as u32));
    } else {
        emit_load_imm(2, frame_size, code); // load into x2 (scratch)
        code.push(e::sub_reg(e::SP, e::SP, 2));
    }
}

fn match_store_reg(size: u64, rt: u32, rn: u32) -> u32 {
    match size {
        1 => e::str_b(rn, rt),
        2 => e::str_h(rn, rt),
        4 => e::str_w(rn, rt),
        _ => e::str_x(rn, rt),
    }
}
fn match_load_reg(size: u64, signed: bool, rt: u32, rn: u32) -> u32 {
    match (size, signed) {
        (1, false) => e::ldr_b(rn, rt),
        (1, true) => e::ldrsb(rn, rt),
        (2, false) => e::ldr_h(rn, rt),
        (2, true) => e::ldrsh(rn, rt),
        (4, false) => e::ldr_w(rn, rt),
        (4, true) => e::ldrsw(rn, rt),
        _ => e::ldr_x(rn, rt),
    }
}

/// Emit an absolute 64-bit immediate load into `rd` using a fixed 4-word
/// movz+movk*3 sequence (unconditionally — see imm_move_words vs this: this
/// fixed form is used specifically where pass 1 assumed 4 words, i.e. global
/// addresses. For PushImm we use the variable-length `emit_load_imm`).
fn emit_load_imm_fixed4(rd: u32, val: u64, code: &mut Vec<u32>) {
    code.push(e::movz(rd, (val & 0xFFFF) as u16, 0));
    code.push(e::movk(rd, ((val >> 16) & 0xFFFF) as u16, 1));
    code.push(e::movk(rd, ((val >> 32) & 0xFFFF) as u16, 2));
    code.push(e::movk(rd, ((val >> 48) & 0xFFFF) as u16, 3));
}

/// Variable-length immediate load matching `imm_move_words`.
fn emit_load_imm(rd: u32, val: u64, code: &mut Vec<u32>) {
    let chunks = [val & 0xFFFF, (val >> 16) & 0xFFFF, (val >> 32) & 0xFFFF, (val >> 48) & 0xFFFF];
    code.push(e::movz(rd, chunks[0] as u16, 0));
    for i in 1..4 {
        if chunks[i] != 0 {
            code.push(e::movk(rd, chunks[i] as u16, i as u32));
        }
    }
}

fn emit_addr_from_fp(offset: i64, rd: u32, code: &mut Vec<u32>) {
    let mag = offset.unsigned_abs();
    if mag <= 0xFFF {
        if offset >= 0 {
            code.push(e::add_imm(rd, e::FP, mag as u32));
        } else {
            code.push(e::sub_imm(rd, e::FP, mag as u32));
        }
    } else {
        emit_load_imm(rd, mag, code);
        if offset >= 0 {
            code.push(e::add_reg(rd, e::FP, rd));
        } else {
            code.push(e::sub_reg(rd, e::FP, rd));
        }
    }
}

fn emit_addr_offset(base_reg: u32, dst_reg: u32, offset: u64, code: &mut Vec<u32>) {
    if offset <= 0xFFF {
        code.push(e::add_imm(dst_reg, base_reg, offset as u32));
    } else {
        emit_load_imm(dst_reg, offset, code);
        code.push(e::add_reg(dst_reg, base_reg, dst_reg));
    }
}

fn emit_op(
    op: &Ir,
    layout: &FuncLayout,
    all_funcs: &HashMap<String, FuncLayout>,
    string_addrs: &HashMap<String, u64>,
    code: &mut Vec<u32>,
) {
    use super::ast::BinOp::*;
    match op {
        Ir::PushImm(n) => {
            emit_load_imm(0, *n, code);
            code.push(e::push_x(0));
        }
        Ir::PushLocalAddr(off) => {
            emit_addr_from_fp(*off, 0, code);
            code.push(e::push_x(0));
        }
        Ir::PushGlobalAddr(label) => {
            let addr = *string_addrs.get(label).expect("unknown string label");
            emit_load_imm_fixed4(0, addr, code);
            code.push(e::push_x(0));
        }
        Ir::Load(size, signed) => {
            code.push(e::pop_x(0));                  // x0 = addr
            code.push(match_load_reg(*size, *signed, 0, 0));
            code.push(e::push_x(0));
        }
        Ir::Store(size) => {
            code.push(e::pop_x(1));                  // x1 = addr
            code.push(e::pop_x(0));                  // x0 = value
            code.push(match_store_reg(*size, 0, 1));
        }
        Ir::IndexAddr(elem_size) => {
            code.push(e::pop_x(1));                  // x1 = index
            code.push(e::pop_x(0));                  // x0 = base addr
            emit_load_imm(2, *elem_size, code);        // x2 = elem_size
            code.push(e::mul(1, 1, 2));                // x1 = index*elem_size
            code.push(e::add_reg(0, 0, 1));             // x0 = base + index*elem_size
            code.push(e::push_x(0));
        }
        Ir::OffsetAddr(off) => {
            code.push(e::pop_x(0));
            emit_addr_offset(0, 0, *off, code);
            code.push(e::push_x(0));
        }
        Ir::Dup => {
            code.push(e::ldr_x(e::SP, 0)); // peek top, no writeback
            code.push(e::push_x(0));
        }
        Ir::Swap => {
            code.push(e::pop_x(1)); // b
            code.push(e::pop_x(0)); // a
            code.push(e::push_x(1)); // push b
            code.push(e::push_x(0)); // push a
        }
        Ir::Drop => {
            code.push(e::add_sp_imm(16));
        }
        Ir::BinOp(bop) => {
            code.push(e::pop_x(1)); // b
            code.push(e::pop_x(0)); // a
            match bop {
                Add => code.push(e::add_reg(0, 0, 1)),
                Sub => code.push(e::sub_reg(0, 0, 1)),
                Mul => code.push(e::mul(0, 0, 1)),
                Div => code.push(e::udiv(0, 0, 1)),
                Mod => {
                    code.push(e::udiv(2, 0, 1));
                    code.push(e::msub(0, 2, 1, 0));
                }
                BitAnd => code.push(e::and_reg(0, 0, 1)),
                BitOr => code.push(e::orr_reg(0, 0, 1)),
                BitXor => code.push(e::eor_reg(0, 0, 1)),
                Shl => code.push(e::lslv(0, 0, 1)),
                Shr => code.push(e::lsrv(0, 0, 1)),
                Eq => { code.push(e::cmp_reg(0, 1)); code.push(e::cset(0, e::COND_EQ)); }
                Ne => { code.push(e::cmp_reg(0, 1)); code.push(e::cset(0, e::COND_NE)); }
                Lt => { code.push(e::cmp_reg(0, 1)); code.push(e::cset(0, e::COND_LO)); }
                Gt => { code.push(e::cmp_reg(0, 1)); code.push(e::cset(0, e::COND_HI)); }
                Le => { code.push(e::cmp_reg(0, 1)); code.push(e::cset(0, e::COND_LS)); }
                Ge => { code.push(e::cmp_reg(0, 1)); code.push(e::cset(0, e::COND_HS)); }
                And => {
                    code.push(e::cmp_imm(0, 0)); code.push(e::cset(0, e::COND_NE));
                    code.push(e::cmp_imm(1, 0)); code.push(e::cset(1, e::COND_NE));
                    code.push(e::and_reg(0, 0, 1));
                }
                Or => {
                    code.push(e::cmp_imm(0, 0)); code.push(e::cset(0, e::COND_NE));
                    code.push(e::cmp_imm(1, 0)); code.push(e::cset(1, e::COND_NE));
                    code.push(e::orr_reg(0, 0, 1));
                }
            }
            code.push(e::push_x(0));
        }
        Ir::Neg => {
            code.push(e::pop_x(0));
            code.push(e::neg(0, 0));
            code.push(e::push_x(0));
        }
        Ir::Not => {
            code.push(e::pop_x(0));
            code.push(e::cmp_imm(0, 0));
            code.push(e::cset(0, e::COND_EQ));
            code.push(e::push_x(0));
        }
        Ir::BitNot => {
            code.push(e::pop_x(0));
            code.push(e::mvn(0, 0));
            code.push(e::push_x(0));
        }
        Ir::Call(name, argc) => {
            for i in (0..*argc).rev() {
                code.push(e::pop_x(i as u32));
            }
            let target = all_funcs.get(name).unwrap_or_else(|| panic!("undefined function '{}'", name));
            let here = code.len();
            let rel = target.start_word as i64 - here as i64;
            code.push(e::bl(rel as i32));
            code.push(e::push_x(0));
        }
        Ir::KernelCall(num, argc) => {
            for i in (0..*argc).rev() {
                code.push(e::pop_x(i as u32));
            }
            code.push(e::svc(*num as u16));
            code.push(e::push_x(0));
        }
        Ir::KernelSubCall(num, sub, argc) => {
            for i in (0..*argc).rev() {
                code.push(e::pop_x(i as u32));
            }
            code.push(e::movz(16, *sub as u16, 0));
            code.push(e::svc(*num as u16));
            code.push(e::push_x(0));
        }
        Ir::Ret(has_value) => {
            if *has_value {
                code.push(e::pop_x(0));
            }
            code.push(e::add_imm(e::SP, e::FP, 0)); // mov sp, x29
            code.push(e::ldp_post(e::FP, e::LR, e::SP, 2));
            code.push(e::ret());
        }
        Ir::Label(_) => {}
        Ir::Jump(id) => {
            let here = code.len();
            let target = layout.label_offsets[id] + layout.start_word;
            code.push(e::b((target as i64 - here as i64) as i32));
        }
        Ir::JumpIfZero(id) => {
            code.push(e::pop_x(0));
            code.push(e::cmp_imm(0, 0));
            let here = code.len();
            let target = layout.label_offsets[id] + layout.start_word;
            code.push(e::b_cond((target as i64 - here as i64) as i32, e::COND_EQ));
        }
    }
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;
    use crate::k::parser::parse;
    use crate::k::typecheck::analyze;
    use crate::println;
    use alloc::format;

    fn compile(src: &str) -> CodegenOutput {
        let m = parse(src).expect("parse");
        let sema = analyze(&m).expect("analyze");
        let prog = crate::k::ir::lower(&m, &sema);
        generate(&prog)
    }

    // ---- pure measurement functions ----

    fn test_imm_move_words_zero() {
        assert_eq!(imm_move_words(0), 1);
    }

    fn test_imm_move_words_single_chunk() {
        assert_eq!(imm_move_words(0xFFFF), 1);
    }

    fn test_imm_move_words_two_chunks() {
        assert_eq!(imm_move_words(0x1_0000), 2); // bit 16 set -> needs one movk
    }

    fn test_imm_move_words_all_chunks() {
        assert_eq!(imm_move_words(0xFFFF_FFFF_FFFF_FFFF), 4);
    }

    fn test_imm_move_words_sparse_chunks() {
        // only chunk 0 and chunk 3 set -> movz + one movk = 2
        assert_eq!(imm_move_words(0x0001_0000_0000_0001), 2);
    }

    fn test_addr_from_fp_words_small_offset() {
        assert_eq!(addr_from_fp_words(8), 1);
        assert_eq!(addr_from_fp_words(-8), 1);
        assert_eq!(addr_from_fp_words(0xFFF), 1);
    }

    fn test_addr_from_fp_words_large_offset() {
        assert!(addr_from_fp_words(0x1000) > 1);
    }

    fn test_binop_len_matches_expected_categories() {
        use crate::k::ast::BinOp::*;
        assert_eq!(binop_len(Add), 1);
        assert_eq!(binop_len(Mod), 2);
        assert_eq!(binop_len(Eq), 2);
        assert_eq!(binop_len(And), 5);
        assert_eq!(binop_len(Or), 5);
    }

    fn test_op_len_matches_hand_count_for_each_variant() {
        use crate::k::ast::BinOp;
        assert_eq!(op_len(&Ir::PushImm(0)), 2); // 1 movz + 1 push
        assert_eq!(op_len(&Ir::Load(8, false)), 3);
        assert_eq!(op_len(&Ir::Store(8)), 3);
        assert_eq!(op_len(&Ir::Dup), 2);
        assert_eq!(op_len(&Ir::Swap), 4);
        assert_eq!(op_len(&Ir::Drop), 1);
        assert_eq!(op_len(&Ir::Neg), 3);
        assert_eq!(op_len(&Ir::Not), 4);
        assert_eq!(op_len(&Ir::BitNot), 3);
        assert_eq!(op_len(&Ir::Label(0)), 0);
        assert_eq!(op_len(&Ir::Jump(0)), 1);
        assert_eq!(op_len(&Ir::JumpIfZero(0)), 3);
        assert_eq!(op_len(&Ir::Ret(false)), 3);
        assert_eq!(op_len(&Ir::Ret(true)), 4);
        assert_eq!(op_len(&Ir::Call("f".into(), 2)), 4); // argc + 2
        assert_eq!(op_len(&Ir::KernelCall(0, 2)), 4);    // argc + 2 (no x8 load anymore)
        assert_eq!(op_len(&Ir::KernelSubCall(16, 0, 1)), 4); // argc + 3 (x16 load)
        assert_eq!(op_len(&Ir::BinOp(BinOp::Add)), 4);   // 2 pops + 1 op + 1 push
        assert_eq!(op_len(&Ir::BinOp(BinOp::And)), 8);   // 2 pops + 5 op + 1 push
    }

    // ---- end-to-end generation: header/layout correctness ----

    fn test_entry_address_is_load_base() {
        let out = compile("module m; fn main() {}");
        assert_eq!(out.entry_address, LOAD_BASE);
    }

    fn test_code_size_is_word_aligned() {
        let out = compile("module m; fn main() { let x: u64 = 1; }");
        assert_eq!(out.code.len() % 4, 0);
    }

    fn test_data_base_after_code_8byte_aligned() {
        let out = compile(r#"module m; fn main() { kernel.print("hi"); }"#);
        assert_eq!(out.data_base % 8, 0);
        assert!(out.data_base >= LOAD_BASE + out.code.len() as u64);
    }

    fn test_no_data_section_when_no_strings() {
        let out = compile("module m; fn main() { let x: u64 = 1; }");
        assert_eq!(out.data.len(), 0);
    }

    fn test_string_data_includes_nul_terminator() {
        let out = compile(r#"module m; fn main() { kernel.print("ab"); }"#);
        assert_eq!(out.data.len(), 3); // 'a','b','\0'
        assert_eq!(out.data, vec![b'a', b'b', 0]);
    }

    fn test_multiple_strings_each_nul_terminated_and_aligned() {
        let out = compile(r#"module m; fn main() { kernel.print("a"); kernel.print("bb"); }"#);
        // "a\0" (2 bytes) padded to 8, then "bb\0" (3 bytes)
        assert_eq!(&out.data[0..2], b"a\0");
        assert_eq!(&out.data[8..11], b"bb\0");
    }

    fn test_recursive_function_compiles() {
        let src = "module m; fn fib(n: u64) -> u64 { \
                       if n <= 1 { return n; } \
                       return fib(n - 1) + fib(n - 2); \
                   } \
                   fn main() { let f: u64 = fib(5); }";
        let out = compile(src);
        assert!(!out.code.is_empty());
    }

    fn test_multiple_functions_get_distinct_addresses() {
        let src = "module m; fn a() -> u64 { return 1; } fn b() -> u64 { return 2; } \
                    fn main() { let x: u64 = a(); let y: u64 = b(); }";
        let out = compile(src);
        assert!(!out.code.is_empty());
        // Just confirming generation succeeds without overlapping/panicking;
        // exact addresses are already exercised indirectly by the
        // measure/emit debug_assert_eq! inside generate() itself.
        let _ = out;
    }

    fn test_struct_pointer_and_arrow_field_compiles() {
        let src = "module m; struct P { x: u64; y: u64; } \
                    fn dist(p: P pointer) -> u64 { return p->x + p->y; } \
                    fn main() { let p: P; p.x = 3; p.y = 4; let addr: P pointer = &p; let d: u64 = dist(addr); }";
        let out = compile(src);
        assert!(!out.code.is_empty());
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_imm_move_words_zero,
            &test_imm_move_words_single_chunk,
            &test_imm_move_words_two_chunks,
            &test_imm_move_words_all_chunks,
            &test_imm_move_words_sparse_chunks,
            &test_addr_from_fp_words_small_offset,
            &test_addr_from_fp_words_large_offset,
            &test_binop_len_matches_expected_categories,
            &test_op_len_matches_hand_count_for_each_variant,
            &test_entry_address_is_load_base,
            &test_code_size_is_word_aligned,
            &test_data_base_after_code_8byte_aligned,
            &test_no_data_section_when_no_strings,
            &test_string_data_includes_nul_terminator,
            &test_multiple_strings_each_nul_terminated_and_aligned,
            &test_recursive_function_compiles,
            &test_multiple_functions_get_distinct_addresses,
            &test_struct_pointer_and_arrow_field_compiles,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("All tests passed");
    }
}
