// encoder.rs — minimal AArch64 instruction encoder.
// Encodes exactly the subset of A64 needed by the K compiler's stack-machine
// codegen. Each function returns one 32-bit little-endian instruction word.

pub const SP: u32 = 31;
pub const XZR: u32 = 31;
pub const FP: u32 = 29; // x29
pub const LR: u32 = 30; // x30

// ---- condition codes (4-bit) ----
pub const COND_EQ: u32 = 0b0000;
pub const COND_NE: u32 = 0b0001;
pub const COND_HS: u32 = 0b0010; // unsigned >=
pub const COND_LO: u32 = 0b0011; // unsigned <
pub const COND_HI: u32 = 0b1000; // unsigned >
pub const COND_LS: u32 = 0b1001; // unsigned <=

pub fn movz(rd: u32, imm16: u16, shift16: u32) -> u32 {
    (1 << 31) | (0b10 << 29) | (0b100101 << 23) | (shift16 << 21) | ((imm16 as u32) << 5) | rd
}
pub fn movk(rd: u32, imm16: u16, shift16: u32) -> u32 {
    (1 << 31) | (0b11 << 29) | (0b100101 << 23) | (shift16 << 21) | ((imm16 as u32) << 5) | rd
}

/// ADD Xd, Xn, #imm12   (imm12 must fit in 12 bits, unshifted)
pub fn add_imm(rd: u32, rn: u32, imm12: u32) -> u32 {
    (1 << 31) | (0 << 30) | (0 << 29) | (0b10001 << 24) | (0 << 22) | ((imm12 & 0xFFF) << 10) | (rn << 5) | rd
}
/// SUB Xd, Xn, #imm12
pub fn sub_imm(rd: u32, rn: u32, imm12: u32) -> u32 {
    (1 << 31) | (1 << 30) | (0 << 29) | (0b10001 << 24) | (0 << 22) | ((imm12 & 0xFFF) << 10) | (rn << 5) | rd
}
/// ADD Xd, Xn, Xm  (register, no shift)
pub fn add_reg(rd: u32, rn: u32, rm: u32) -> u32 {
    (1 << 31) | (0 << 30) | (0 << 29) | (0b01011 << 24) | (0 << 22) | (0 << 21) | (rm << 16) | (0 << 10) | (rn << 5) | rd
}
/// SUB Xd, Xn, Xm
pub fn sub_reg(rd: u32, rn: u32, rm: u32) -> u32 {
    (1 << 31) | (1 << 30) | (0 << 29) | (0b01011 << 24) | (0 << 22) | (0 << 21) | (rm << 16) | (0 << 10) | (rn << 5) | rd
}
/// NEG Xd, Xm  == SUB Xd, XZR, Xm
pub fn neg(rd: u32, rm: u32) -> u32 { sub_reg(rd, XZR, rm) }

/// MUL Xd, Xn, Xm == MADD Xd, Xn, Xm, XZR
pub fn mul(rd: u32, rn: u32, rm: u32) -> u32 {
    (1 << 31) | (0 << 29) | (0b11011 << 24) | (0b000 << 21) | (rm << 16) | (0 << 15) | (XZR << 10) | (rn << 5) | rd
}
/// MSUB Xd, Xn, Xm, Ra  => Xd = Ra - Xn*Xm
pub fn msub(rd: u32, rn: u32, rm: u32, ra: u32) -> u32 {
    (1 << 31) | (0 << 29) | (0b11011 << 24) | (0b000 << 21) | (rm << 16) | (1 << 15) | (ra << 10) | (rn << 5) | rd
}
pub fn udiv(rd: u32, rn: u32, rm: u32) -> u32 {
    (1 << 31) | (0 << 30) | (0 << 29) | (0b11010110 << 21) | (rm << 16) | (0b000010 << 10) | (rn << 5) | rd
}
#[allow(dead_code)]
pub fn sdiv(rd: u32, rn: u32, rm: u32) -> u32 {
    (1 << 31) | (0 << 30) | (0 << 29) | (0b11010110 << 21) | (rm << 16) | (0b000011 << 10) | (rn << 5) | rd
}

fn logical_reg(opc: u32, rd: u32, rn: u32, rm: u32) -> u32 {
    (1 << 31) | (opc << 29) | (0b01010 << 24) | (0 << 22) | (0 << 21) | (rm << 16) | (0 << 10) | (rn << 5) | rd
}
pub fn and_reg(rd: u32, rn: u32, rm: u32) -> u32 { logical_reg(0b00, rd, rn, rm) }
pub fn orr_reg(rd: u32, rn: u32, rm: u32) -> u32 { logical_reg(0b01, rd, rn, rm) }
pub fn eor_reg(rd: u32, rn: u32, rm: u32) -> u32 { logical_reg(0b10, rd, rn, rm) }
/// MVN Xd, Xm == ORN Xd, XZR, Xm  (opc=01, N=1)
pub fn mvn(rd: u32, rm: u32) -> u32 {
    (1 << 31) | (0b01 << 29) | (0b01010 << 24) | (0 << 22) | (1 << 21) | (rm << 16) | (0 << 10) | (XZR << 5) | rd
}

fn shift_reg(opcode: u32, rd: u32, rn: u32, rm: u32) -> u32 {
    (1 << 31) | (0 << 30) | (0 << 29) | (0b11010110 << 21) | (rm << 16) | (opcode << 10) | (rn << 5) | rd
}
pub fn lslv(rd: u32, rn: u32, rm: u32) -> u32 { shift_reg(0b001000, rd, rn, rm) }
pub fn lsrv(rd: u32, rn: u32, rm: u32) -> u32 { shift_reg(0b001001, rd, rn, rm) }

/// CMP Xn, Xm  == SUBS XZR, Xn, Xm
pub fn cmp_reg(rn: u32, rm: u32) -> u32 {
    (1 << 31) | (1 << 30) | (1 << 29) | (0b01011 << 24) | (0 << 22) | (0 << 21) | (rm << 16) | (0 << 10) | (rn << 5) | XZR
}
/// CMP Xn, #imm12  == SUBS XZR, Xn, #imm
pub fn cmp_imm(rn: u32, imm12: u32) -> u32 {
    (1 << 31) | (1 << 30) | (1 << 29) | (0b10001 << 24) | (0 << 22) | ((imm12 & 0xFFF) << 10) | (rn << 5) | XZR
}
/// CSET Xd, cond  == CSINC Xd, XZR, XZR, invert(cond)
pub fn cset(rd: u32, cond: u32) -> u32 {
    let inv = cond ^ 1; // valid for all paired conditions used by this compiler
    (1 << 31) | (0 << 30) | (0 << 29) | (0b11010100 << 21) | (XZR << 16) | (inv << 12) | (0b01 << 10) | (XZR << 5) | rd
}

// ---- LDR/STR (unsigned offset, no writeback); offset is ALWAYS 0 in this
// compiler (addresses are pre-computed into a register beforehand). ----
fn ldst_unsigned_off0(size: u32, opc: u32, rn: u32, rt: u32) -> u32 {
    (size << 30) | (0b111 << 27) | (0 << 26) | (0b01 << 24) | (opc << 22) | (0 << 10) | (rn << 5) | rt
}
pub fn str_x(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b11, 0b00, rn, rt) }
pub fn ldr_x(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b11, 0b01, rn, rt) }
pub fn str_w(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b10, 0b00, rn, rt) }
pub fn ldr_w(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b10, 0b01, rn, rt) } // zero-extend
pub fn str_h(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b01, 0b00, rn, rt) }
pub fn ldr_h(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b01, 0b01, rn, rt) } // zero-extend
pub fn str_b(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b00, 0b00, rn, rt) }
pub fn ldr_b(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b00, 0b01, rn, rt) } // zero-extend
pub fn ldrsw(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b10, 0b10, rn, rt) } // sign-extend 32->64
pub fn ldrsh(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b01, 0b10, rn, rt) } // sign-extend 16->64
pub fn ldrsb(rn: u32, rt: u32) -> u32 { ldst_unsigned_off0(0b00, 0b10, rn, rt) } // sign-extend 8->64

/// STR Xt, [SP, #-16]!  (pre-index, push)
pub fn push_x(rt: u32) -> u32 {
    let imm9 = (-16i32) & 0x1FF;
    (0b11 << 30) | (0b111 << 27) | (0 << 26) | (0b00 << 24) | (0b00 << 22) | ((imm9 as u32) << 12) | (0b11 << 10) | (SP << 5) | rt
}
/// LDR Xt, [SP], #16  (post-index, pop)
pub fn pop_x(rt: u32) -> u32 {
    let imm9: u32 = 16;
    (0b11 << 30) | (0b111 << 27) | (0 << 26) | (0b00 << 24) | (0b01 << 22) | (imm9 << 12) | (0b01 << 10) | (SP << 5) | rt
}

/// ADD SP, SP, #imm12  (drop n*16 bytes; imm must be multiple of 16, <=4095)
pub fn add_sp_imm(imm12: u32) -> u32 { add_imm(SP, SP, imm12) }

/// STP X_t1, X_t2, [SP, #-16]!
pub fn stp_pre(rt1: u32, rt2: u32, rn: u32, imm7_words: i32) -> u32 {
    let imm7 = (imm7_words as u32) & 0x7F;
    (0b10 << 30) | (0b101 << 27) | (0 << 26) | (0b011 << 23) | (0 << 22) | (imm7 << 15) | (rt2 << 10) | (rn << 5) | rt1
}
/// LDP X_t1, X_t2, [SP], #imm
pub fn ldp_post(rt1: u32, rt2: u32, rn: u32, imm7_words: i32) -> u32 {
    let imm7 = (imm7_words as u32) & 0x7F;
    (0b10 << 30) | (0b101 << 27) | (0 << 26) | (0b001 << 23) | (1 << 22) | (imm7 << 15) | (rt2 << 10) | (rn << 5) | rt1
}

pub fn b(imm26_words: i32) -> u32 {
    (0b000101 << 26) | ((imm26_words as u32) & 0x3FFFFFF)
}
pub fn bl(imm26_words: i32) -> u32 {
    (0b100101 << 26) | ((imm26_words as u32) & 0x3FFFFFF)
}
pub fn b_cond(imm19_words: i32, cond: u32) -> u32 {
    (0b01010100 << 24) | (((imm19_words as u32) & 0x7FFFF) << 5) | cond
}
pub fn ret() -> u32 { 0xD65F03C0 }
/// SVC #imm16 — the syscall number is the trapped instruction's own
/// immediate field (read back by the kernel via ESR_EL1), not a register.
pub fn svc(imm16: u16) -> u32 { 0xD4000001 | ((imm16 as u32) << 5) }
pub fn svc0() -> u32 { svc(0) }
#[allow(dead_code)]
pub fn nop() -> u32 { 0xD503201F }

#[cfg(feature = "test")]
pub mod tests {
    //! Every expected value below was independently verified by emitting
    //! the word, disassembling it with `aarch64-linux-gnu-objdump`, and
    //! confirming the resulting mnemonic and operands match. See the
    //! commit/PR history (or re-run `examples/dump_encodings.rs` piped
    //! through objdump) to reproduce that verification.
    use crate::println;
    use super::*;

    fn test_movz_movk() {
        assert_eq!(movz(0, 0x1234, 0), 0xd2824680);
        assert_eq!(movk(1, 0x5678, 1), 0xf2aacf01);
    }

    fn test_add_sub_imm() {
        assert_eq!(add_imm(0, 1, 5), 0x91001420);
        assert_eq!(sub_imm(0, 1, 5), 0xd1001420);
    }

    fn test_add_sub_reg() {
        assert_eq!(add_reg(0, 1, 2), 0x8b020020);
        assert_eq!(sub_reg(0, 1, 2), 0xcb020020);
        assert_eq!(neg(0, 1), 0xcb0103e0);
    }

    fn test_mul_div() {
        assert_eq!(mul(0, 1, 2), 0x9b027c20);
        assert_eq!(msub(0, 1, 2, 3), 0x9b028c20);
        assert_eq!(udiv(0, 1, 2), 0x9ac20820);
        assert_eq!(sdiv(0, 1, 2), 0x9ac20c20);
    }

    fn test_logical() {
        assert_eq!(and_reg(0, 1, 2), 0x8a020020);
        assert_eq!(orr_reg(0, 1, 2), 0xaa020020);
        assert_eq!(eor_reg(0, 1, 2), 0xca020020);
        assert_eq!(mvn(0, 1), 0xaa2103e0);
    }

    fn test_shifts() {
        assert_eq!(lslv(0, 1, 2), 0x9ac22020);
        assert_eq!(lsrv(0, 1, 2), 0x9ac22420);
    }

    fn test_compare_and_set() {
        assert_eq!(cmp_reg(0, 1), 0xeb01001f);
        assert_eq!(cmp_imm(0, 7), 0xf1001c1f);
        assert_eq!(cset(0, COND_EQ), 0x9a9f17e0);
        assert_eq!(cset(0, COND_NE), 0x9a9f07e0);
        assert_eq!(cset(0, COND_LO), 0x9a9f27e0); // alias `cc`
        assert_eq!(cset(0, COND_HI), 0x9a9f97e0);
        assert_eq!(cset(0, COND_LS), 0x9a9f87e0);
        assert_eq!(cset(0, COND_HS), 0x9a9f37e0); // alias `cs`
    }

    fn test_load_store_sizes() {
        assert_eq!(str_x(1, 0), 0xf9000020);
        assert_eq!(ldr_x(1, 0), 0xf9400020);
        assert_eq!(str_w(1, 0), 0xb9000020);
        assert_eq!(ldr_w(1, 0), 0xb9400020);
        assert_eq!(str_h(1, 0), 0x79000020);
        assert_eq!(ldr_h(1, 0), 0x79400020);
        assert_eq!(str_b(1, 0), 0x39000020);
        assert_eq!(ldr_b(1, 0), 0x39400020);
        assert_eq!(ldrsw(1, 0), 0xb9800020);
        assert_eq!(ldrsh(1, 0), 0x79800020);
        assert_eq!(ldrsb(1, 0), 0x39800020);
    }

    fn test_stack_push_pop() {
        assert_eq!(push_x(0), 0xf81f0fe0);
        assert_eq!(pop_x(0), 0xf84107e0);
        assert_eq!(add_sp_imm(16), 0x910043ff);
    }

    fn test_frame_pair_ops() {
        assert_eq!(stp_pre(29, 30, SP, -2), 0xa9bf7bfd);
        assert_eq!(ldp_post(29, 30, SP, 2), 0xa8c17bfd);
    }

    fn test_branches_and_exceptions() {
        assert_eq!(b(2), 0x14000002);
        assert_eq!(bl(2), 0x94000002);
        assert_eq!(b_cond(2, COND_EQ), 0x54000040);
        assert_eq!(ret(), 0xd65f03c0);
        assert_eq!(svc(0), 0xd4000001);
        assert_eq!(svc(5), 0xd40000a1);
        assert_eq!(svc0(), svc(0));
        assert_eq!(nop(), 0xd503201f);
    }

    fn test_condition_code_pairing_invariant() {
        assert_eq!(COND_EQ ^ 1, COND_NE);
        assert_eq!(COND_HS ^ 1, COND_LO);
        assert_eq!(COND_HI ^ 1, COND_LS);
    }

    fn test_registers_distinct_from_encodable_range() {
        for r in [SP, XZR, FP, LR] {
            assert!(r <= 31);
        }
        assert_eq!(SP, 31);
        assert_eq!(XZR, 31);
        assert_eq!(FP, 29);
        assert_eq!(LR, 30);
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_movz_movk,
            &test_add_sub_imm,
            &test_add_sub_reg,
            &test_mul_div,
            &test_logical,
            &test_shifts,
            &test_compare_and_set,
            &test_load_store_sizes,
            &test_stack_push_pop,
            &test_frame_pair_ops,
            &test_branches_and_exceptions,
            &test_condition_code_pairing_invariant,
            &test_registers_distinct_from_encodable_range,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("OK");
    }
}
