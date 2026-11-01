//! ir.rs — a small stack-based intermediate representation.
//!
//! Execution model: every IR program manipulates one operand stack (which,
//! at codegen time, is literally the AArch64 hardware stack via SP). There
//! is no register allocator — this mirrors the language's "fully stack
//! based" evaluation model end-to-end from source to machine code.
//!
//! Store convention: `Store(size)` expects the stack, bottom-to-top, to be
//! [value, addr] i.e. `value` was pushed, then `addr` was pushed on top.
//! It pops addr, pops value, writes value to *addr.

use super::ast::*;
use super::typecheck::{field_size, Sema};
use hashbrown::HashMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::string::ToString;
use alloc::vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Ir {
    PushImm(u64),
    PushLocalAddr(i64),          // push (frame_base + offset) — frame_base = x29
    PushGlobalAddr(String),      // push address of a static-data label
    Load(u64 /*size*/, bool /*signed*/), // pop addr -> push *addr
    Store(u64 /*size*/),         // pop addr, pop value -> *addr = value
    IndexAddr(u64 /*elem_size*/),// pop index, pop base_addr -> push base_addr+index*elem_size
    OffsetAddr(u64 /*offset*/),  // pop addr -> push addr+offset  (struct field)
    Dup,
    Swap,
    Drop,
    BinOp(BinOp),
    Neg, Not, BitNot,
    Call(String, usize /*argc*/),
    KernelCall(u8 /*syscall#*/, usize /*argc*/),
    Ret(bool /*has_value: a value was pushed and must be popped into x0*/),
    Label(usize),
    Jump(usize),
    JumpIfZero(usize),
}

pub struct IrFunction {
    pub name: String,
    pub frame_size: u64,       // bytes reserved for locals (16-byte aligned)
    pub param_count: usize,
    pub params: Vec<Type>,
    pub param_offsets: Vec<i64>, // frame offset (from x29) for each parameter slot
    pub body: Vec<Ir>,
}

pub struct IrProgram {
    pub functions: Vec<IrFunction>,
    pub strings: Vec<(String /*label*/, String /*bytes*/)>,
}

// Syscall numbers here are the `svc #N` immediate itself (see
// aarch64_codegen.rs::emit_op — Ir::KernelCall emits `svc #num` directly,
// no register carries the number). Matches the kernel's `#[syscall(N)]`
// table. `filesystem` (5) is intentionally NOT listed: it multiplexes a
// sub-command through x16, which this single-number KernelCall shape can't
// express yet — see `Expr::KernelCall` lowering below for how a dedicated
// filesystem op could be added.
pub const KERNEL_SYSCALLS: &[(&str, u8)] = &[
    ("exit", 0),        // no args read by the handler, but harmless to pass one
    ("write_byte", 1),  // (byte: u8)
    ("write_bytes", 2), // (ptr: u8 pointer, len: u64)
    ("read_line", 3),   // (ptr: u8 pointer, max_len: u64) -> u64 (bytes read)
    ("dump", 8),        //
];

struct FnCtx<'a> {
    sema: &'a Sema,
    locals: Vec<HashMap<String, (i64 /*frame offset*/, Type)>>,
    next_offset: i64,   // grows negative from x29
    max_offset: i64,    // most negative reached (frame size need)
    label_counter: usize,
    loop_stack: Vec<(usize /*continue label*/, usize /*break label*/)>,
    strings: &'a mut Vec<(String, String)>,
    string_counter: &'a mut usize,
    tmp_counter: usize,
}

impl<'a> FnCtx<'a> {
    fn new_label(&mut self) -> usize { self.label_counter += 1; self.label_counter }

    fn push_scope(&mut self) { self.locals.push(HashMap::new()); }
    fn pop_scope(&mut self) { self.locals.pop(); }

    fn define_local(&mut self, name: &str, ty: &Type) -> i64 {
        let size = field_size(self.sema, ty).max(1);
        let aligned = (size + 7) & !7; // 8-byte align each slot
        self.next_offset -= aligned as i64;
        if self.next_offset < self.max_offset { self.max_offset = self.next_offset; }
        self.locals.last_mut().unwrap().insert(name.to_string(), (self.next_offset, ty.clone()));
        self.next_offset
    }

    fn new_tmp(&mut self, ty: &Type) -> i64 {
        self.tmp_counter += 1;
        let name = format!("__tmp{}", self.tmp_counter);
        self.define_local(&name, ty)
    }

    fn lookup_local(&self, name: &str) -> Option<(i64, Type)> {
        for scope in self.locals.iter().rev() {
            if let Some((off, ty)) = scope.get(name) { return Some((*off, ty.clone())); }
        }
        None
    }

    fn intern_string(&mut self, s: &str) -> String {
        *self.string_counter += 1;
        let label = format!("__str{}", self.string_counter);
        self.strings.push((label.clone(), s.to_string()));
        label
    }
}

pub fn lower(module: &Module, sema: &Sema) -> IrProgram {
    let mut strings = Vec::new();
    let mut string_counter = 0usize;
    let mut functions = Vec::new();

    for item in &module.items {
        if let Item::Function(f) = item {
            functions.push(lower_function(f, sema, &mut strings, &mut string_counter));
        }
    }

    IrProgram { functions, strings }
}

fn lower_function(f: &Function, sema: &Sema, strings: &mut Vec<(String, String)>, string_counter: &mut usize) -> IrFunction {
    let mut ctx = FnCtx {
        sema,
        locals: vec![HashMap::new()],
        next_offset: 0,
        max_offset: 0,
        label_counter: 0,
        loop_stack: Vec::new(),
        strings,
        string_counter,
        tmp_counter: 0,
    };

    let mut body = Vec::new();

    // Parameters arrive in x0..x7 (spilled to frame slots by the codegen
    // prologue); reserve their local slots here so offsets line up.
    let mut param_offsets = Vec::new();
    for (pname, pty) in &f.params {
        param_offsets.push(ctx.define_local(pname, pty));
    }

    lower_block(&f.body, &mut ctx, &mut body);

    // Implicit return for void functions falling off the end.
    body.push(Ir::Ret(false));

    let frame_size = ((-ctx.max_offset) as u64 + 15) & !15; // 16-byte align

    IrFunction {
        name: f.name.clone(),
        frame_size,
        param_count: f.params.len(),
        params: f.params.iter().map(|(_, t)| t.clone()).collect(),
        param_offsets,
        body,
    }
}

fn lower_block(blk: &Block, ctx: &mut FnCtx, out: &mut Vec<Ir>) {
    ctx.push_scope();
    for stmt in blk {
        lower_stmt(stmt, ctx, out);
    }
    ctx.pop_scope();
}

fn lower_stmt(stmt: &Stmt, ctx: &mut FnCtx, out: &mut Vec<Ir>) {
    match stmt {
        Stmt::Let { name, ty, init, .. } => {
            let off = ctx.define_local(name, ty);
            if let Some(e) = init {
                if let Expr::ArrayLit(items) = e {
                    let elem_ty = match ty { Type::Array(t, _) => (**t).clone(), _ => Type::U64 };
                    let elem_size = field_size(ctx.sema, &elem_ty).max(1);
                    for (idx, item) in items.iter().enumerate() {
                        lower_expr(item, ctx, out);                    // [value]
                        out.push(Ir::PushLocalAddr(off));                // [value, base_addr]
                        out.push(Ir::OffsetAddr(idx as u64 * elem_size));// [value, elem_addr]
                        out.push(Ir::Store(elem_size));
                    }
                } else {
                    let size = field_size(ctx.sema, ty).max(1);
                    lower_expr(e, ctx, out);            // [value]
                    out.push(Ir::PushLocalAddr(off));    // [value, addr]
                    out.push(Ir::Store(size));
                }
            }
        }
        Stmt::Assign { target, value } => {
            let ty = lvalue_type(target, ctx);
            let size = field_size(ctx.sema, &ty).max(1);
            lower_expr(value, ctx, out);             // [value]
            lower_lvalue_addr(target, ctx, out);      // [value, addr]
            out.push(Ir::Store(size));
        }
        Stmt::ExprStmt(e) => {
            lower_expr(e, ctx, out);
            out.push(Ir::Drop);
        }
        Stmt::Return(e) => {
            let has_value = e.is_some();
            if let Some(e) = e { lower_expr(e, ctx, out); }
            out.push(Ir::Ret(has_value));
        }
        Stmt::If { cond, then_blk, else_blk } => {
            lower_expr(cond, ctx, out);
            let else_lbl = ctx.new_label();
            let end_lbl = ctx.new_label();
            out.push(Ir::JumpIfZero(else_lbl));
            lower_block(then_blk, ctx, out);
            out.push(Ir::Jump(end_lbl));
            out.push(Ir::Label(else_lbl));
            if let Some(eb) = else_blk { lower_block(eb, ctx, out); }
            out.push(Ir::Label(end_lbl));
        }
        Stmt::While { cond, body } => {
            let start_lbl = ctx.new_label();
            let end_lbl = ctx.new_label();
            out.push(Ir::Label(start_lbl));
            lower_expr(cond, ctx, out);
            out.push(Ir::JumpIfZero(end_lbl));
            ctx.loop_stack.push((start_lbl, end_lbl));
            lower_block(body, ctx, out);
            ctx.loop_stack.pop();
            out.push(Ir::Jump(start_lbl));
            out.push(Ir::Label(end_lbl));
        }
        Stmt::Loop { body } => {
            let start_lbl = ctx.new_label();
            let end_lbl = ctx.new_label();
            out.push(Ir::Label(start_lbl));
            ctx.loop_stack.push((start_lbl, end_lbl));
            lower_block(body, ctx, out);
            ctx.loop_stack.pop();
            out.push(Ir::Jump(start_lbl));
            out.push(Ir::Label(end_lbl));
        }
        Stmt::Break => {
            let (_, end) = *ctx.loop_stack.last().expect("break outside loop");
            out.push(Ir::Jump(end));
        }
        Stmt::Continue => {
            let (start, _) = *ctx.loop_stack.last().expect("continue outside loop");
            out.push(Ir::Jump(start));
        }
        Stmt::Match { scrutinee, arms } => {
            let scrut_off = ctx.new_tmp(&Type::U64);
            lower_expr(scrutinee, ctx, out);         // [value]
            out.push(Ir::PushLocalAddr(scrut_off));   // [value, addr]
            out.push(Ir::Store(8));

            let end_lbl = ctx.new_label();
            let mut next_lbl = ctx.new_label();
            for (i, (variant, blk)) in arms.iter().enumerate() {
                out.push(Ir::PushLocalAddr(scrut_off));
                out.push(Ir::Load(8, false));
                let disc = variant_discriminant(ctx.sema, variant);
                out.push(Ir::PushImm(disc));
                out.push(Ir::BinOp(BinOp::Eq));
                out.push(Ir::JumpIfZero(next_lbl));
                lower_block(blk, ctx, out);
                out.push(Ir::Jump(end_lbl));
                out.push(Ir::Label(next_lbl));
                if i != arms.len() - 1 { next_lbl = ctx.new_label(); }
            }
            out.push(Ir::Label(end_lbl));
        }
    }
}

fn variant_discriminant(sema: &Sema, variant: &str) -> u64 {
    for info in sema.enums.values() {
        if let Some(idx) = info.variants.iter().position(|v| v == variant) {
            return idx as u64;
        }
    }
    0
}

/// The static type of an lvalue expression (used to size stores/loads).
fn lvalue_type(e: &Expr, ctx: &FnCtx) -> Type {
    match e {
        Expr::Ident(name) => ctx.lookup_local(name).map(|(_, t)| t).unwrap_or(Type::U64),
        Expr::Deref(inner) => {
            match expr_static_type(inner, ctx) {
                Type::Pointer(t) => *t,
                _ => Type::U64,
            }
        }
        Expr::Index(base, _) => expr_elem_type(base, ctx),
        Expr::Field(base, fname) => {
            let sname = struct_name_of(base, ctx);
            field_info(ctx.sema, &sname, fname).1
        }
        Expr::ArrowField(base, fname) => {
            let sname = struct_name_of_pointer(base, ctx);
            field_info(ctx.sema, &sname, fname).1
        }
        _ => Type::U64,
    }
}

fn expr_static_type(e: &Expr, ctx: &FnCtx) -> Type {
    match e {
        Expr::Ident(name) => ctx.lookup_local(name).map(|(_, t)| t).unwrap_or(Type::U64),
        _ => Type::U64,
    }
}

/// Push the address of an lvalue expression onto the operand stack.
fn lower_lvalue_addr(e: &Expr, ctx: &mut FnCtx, out: &mut Vec<Ir>) {
    match e {
        Expr::Ident(name) => {
            let (off, _) = ctx.lookup_local(name).expect("undefined variable");
            out.push(Ir::PushLocalAddr(off));
        }
        Expr::Deref(inner) => { lower_expr(inner, ctx, out); }
        Expr::Index(base, idx) => {
            let elem_ty = expr_elem_type(base, ctx);
            let elem_size = field_size(ctx.sema, &elem_ty).max(1);
            lower_base_addr(base, ctx, out);
            lower_expr(idx, ctx, out);
            out.push(Ir::IndexAddr(elem_size));
        }
        Expr::Field(base, fname) => {
            let sname = struct_name_of(base, ctx);
            let (offset, _) = field_info(ctx.sema, &sname, fname);
            lower_base_addr(base, ctx, out);
            out.push(Ir::OffsetAddr(offset));
        }
        Expr::ArrowField(base, fname) => {
            let sname = struct_name_of_pointer(base, ctx);
            let (offset, _) = field_info(ctx.sema, &sname, fname);
            lower_expr(base, ctx, out); // base is a pointer value
            out.push(Ir::OffsetAddr(offset));
        }
        _ => panic!("not a valid lvalue: {:?}", e),
    }
}

/// For array/struct bases stored inline, the "address" of the base is its
/// lvalue address. For pointer-typed bases, the pointer's *value* is the
/// address to use (e.g. `p[i]` where `p: T pointer`).
fn lower_base_addr(base: &Expr, ctx: &mut FnCtx, out: &mut Vec<Ir>) {
    if let Expr::Ident(name) = base {
        if let Some((_, Type::Pointer(_))) = ctx.lookup_local(name) {
            lower_expr(base, ctx, out);
            return;
        }
    }
    lower_lvalue_addr(base, ctx, out);
}

fn expr_elem_type(base: &Expr, ctx: &FnCtx) -> Type {
    if let Expr::Ident(name) = base {
        if let Some((_, ty)) = ctx.lookup_local(name) {
            return match ty {
                Type::Array(t, _) => *t,
                Type::Pointer(t) => *t,
                other => other,
            };
        }
    }
    Type::U64
}

fn struct_name_of(base: &Expr, ctx: &FnCtx) -> String {
    if let Expr::Ident(name) = base {
        if let Some((_, ty)) = ctx.lookup_local(name) {
            if let Type::Named(n) = ty { return n; }
        }
    }
    if let Expr::Field(inner_base, fname) = base {
        let sname = struct_name_of(inner_base, ctx);
        if let (_, Type::Named(n)) = field_info(ctx.sema, &sname, fname) { return n; }
    }
    String::new()
}

fn struct_name_of_pointer(base: &Expr, ctx: &FnCtx) -> String {
    if let Expr::Ident(name) = base {
        if let Some((_, ty)) = ctx.lookup_local(name) {
            if let Type::Pointer(inner) = ty {
                if let Type::Named(n) = *inner { return n; }
            }
        }
    }
    String::new()
}

fn field_info(sema: &Sema, struct_name: &str, field: &str) -> (u64, Type) {
    if let Some(si) = sema.structs.get(struct_name) {
        for (fname, fty, off) in &si.fields {
            if fname == field { return (*off, fty.clone()); }
        }
    }
    (0, Type::U64)
}

fn lower_expr(e: &Expr, ctx: &mut FnCtx, out: &mut Vec<Ir>) {
    match e {
        Expr::IntLit(n) => out.push(Ir::PushImm(*n)),
        Expr::BoolLit(b) => out.push(Ir::PushImm(if *b { 1 } else { 0 })),
        Expr::CharLit(c) => out.push(Ir::PushImm(*c as u64)),
        Expr::StrLit(s) => {
            let label = ctx.intern_string(s);
            out.push(Ir::PushGlobalAddr(label));
        }
        Expr::Ident(name) => {
            let (off, ty) = ctx.lookup_local(name).unwrap_or_else(|| panic!("undefined variable '{}'", name));
            out.push(Ir::PushLocalAddr(off));
            match ty {
                Type::Array(_, _) => { /* arrays decay: address IS the value */ }
                _ => {
                    let size = field_size(ctx.sema, &ty).max(1);
                    out.push(Ir::Load(size, ty.is_signed()));
                }
            }
        }
        Expr::ArrayLit(_items) => {
            panic!("array literals are only supported as `let` initializers in this version");
        }
        Expr::Index(base, _idx) => {
            let elem_ty = expr_elem_type(base, ctx);
            let size = field_size(ctx.sema, &elem_ty).max(1);
            lower_lvalue_addr(e, ctx, out);
            out.push(Ir::Load(size, elem_ty.is_signed()));
        }
        Expr::Field(base, fname) => {
            let sname = struct_name_of(base, ctx);
            let (_, fty) = field_info(ctx.sema, &sname, fname);
            let size = field_size(ctx.sema, &fty).max(1);
            lower_lvalue_addr(e, ctx, out);
            out.push(Ir::Load(size, fty.is_signed()));
        }
        Expr::ArrowField(base, fname) => {
            let sname = struct_name_of_pointer(base, ctx);
            let (_, fty) = field_info(ctx.sema, &sname, fname);
            let size = field_size(ctx.sema, &fty).max(1);
            lower_lvalue_addr(e, ctx, out);
            out.push(Ir::Load(size, fty.is_signed()));
        }
        Expr::Unary(op, inner) => {
            lower_expr(inner, ctx, out);
            match op {
                UnOp::Neg => out.push(Ir::Neg),
                UnOp::Not => out.push(Ir::Not),
                UnOp::BitNot => out.push(Ir::BitNot),
            }
        }
        Expr::Binary(op, l, r) => {
            lower_expr(l, ctx, out);
            lower_expr(r, ctx, out);
            out.push(Ir::BinOp(*op));
        }
        Expr::Call(name, args) => {
            for a in args { lower_expr(a, ctx, out); }
            out.push(Ir::Call(name.clone(), args.len()));
        }
        Expr::KernelCall(name, args) => {
            if name == "print" {
                // Not a real syscall — expand to write_bytes(ptr, len) using
                // the string literal's compile-time-known length. There's no
                // runtime strlen loop (yet), so only literals are supported.
                match &args[0] {
                    Expr::StrLit(s) => {
                        let label = ctx.intern_string(s);
                        out.push(Ir::PushGlobalAddr(label));
                        out.push(Ir::PushImm(s.len() as u64));
                        let write_bytes_num = KERNEL_SYSCALLS.iter()
                            .find(|(n, _)| *n == "write_bytes").map(|(_, n)| *n)
                            .expect("KERNEL_SYSCALLS must define 'write_bytes'");
                        out.push(Ir::KernelCall(write_bytes_num, 2));
                    }
                    _ => panic!("kernel.print() only supports string literal arguments in this version — use kernel.write_bytes(ptr, len) for anything else"),
                }
            } else {
                for a in args { lower_expr(a, ctx, out); }
                let num = KERNEL_SYSCALLS.iter().find(|(n, _)| *n == name).map(|(_, n)| *n)
                    .unwrap_or_else(|| panic!("unknown kernel function '{}'", name));
                out.push(Ir::KernelCall(num, args.len()));
            }
        }
        Expr::AddrOf(inner) => { lower_lvalue_addr(inner, ctx, out); }
        Expr::Deref(inner) => {
            lower_expr(inner, ctx, out);
            out.push(Ir::Load(8, false));
        }
        Expr::EnumVariant(_, variant) => {
            out.push(Ir::PushImm(variant_discriminant(ctx.sema, variant)));
        }
        Expr::Sizeof(ty) => { out.push(Ir::PushImm(field_size(ctx.sema, ty).max(1))); }
        Expr::PostIncr(inner) | Expr::PostDecr(inner) => {
            let is_incr = matches!(e, Expr::PostIncr(_));
            let ty = lvalue_type(inner, ctx);
            let size = field_size(ctx.sema, &ty).max(1);
            let signed = ty.is_signed();

            // result = old value (left on the stack at the end)
            lower_lvalue_addr(inner, ctx, out);       // [addr1]
            out.push(Ir::Load(size, signed));          // [old]

            // side effect: *addr = old (op) 1
            lower_lvalue_addr(inner, ctx, out);        // [old, addr2]
            out.push(Ir::Dup);                           // [old, addr2, addr2]
            out.push(Ir::Load(size, signed));           // [old, addr2, cur]
            out.push(Ir::PushImm(1));                    // [old, addr2, cur, 1]
            out.push(Ir::BinOp(if is_incr { BinOp::Add } else { BinOp::Sub })); // [old, addr2, new]
            out.push(Ir::Swap);                           // [old, new, addr2]
            out.push(Ir::Store(size));                    // [old]
        }
    }
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;
    use crate::k::parser::parse;
    use crate::k::typecheck::analyze;
    use crate::println;

    fn lower_fn(src: &str, fn_name: &str) -> Vec<Ir> {
        let m = parse(src).expect("parse");
        let sema = analyze(&m).expect("analyze");
        let prog = lower(&m, &sema);
        prog.functions.into_iter().find(|f| f.name == fn_name)
            .unwrap_or_else(|| panic!("no function named '{}'", fn_name)).body
    }

    fn lower_main(src: &str) -> Vec<Ir> { lower_fn(src, "main") }

    fn test_empty_function_just_returns() {
        let body = lower_main("module m; fn main() {}");
        assert_eq!(body, vec![Ir::Ret(false)]);
    }

    fn test_let_with_init_pushes_value_then_addr_then_stores() {
        let body = lower_main("module m; fn main() { let x: u64 = 5; }");
        assert_eq!(&body[0..3], &[
            Ir::PushImm(5),
            Ir::PushLocalAddr(-8),
            Ir::Store(8),
        ]);
    }

    fn test_let_without_init_emits_nothing() {
        let body = lower_main("module m; fn main() { let x: u64; }");
        assert_eq!(body, vec![Ir::Ret(false)]);
    }

    fn test_assign_pushes_value_then_addr_then_stores() {
        let body = lower_main("module m; fn main() { mut x: u64 = 1; x = 2; }");
        let assign_start = body.iter().position(|op| matches!(op, Ir::PushImm(2))).unwrap();
        assert_eq!(&body[assign_start..assign_start + 3], &[
            Ir::PushImm(2),
            Ir::PushLocalAddr(-8),
            Ir::Store(8),
        ]);
    }

    fn test_locals_get_sequential_negative_offsets() {
        let body = lower_main("module m; fn main() { let a: u64 = 1; let b: u64 = 2; }");
        assert!(body.contains(&Ir::PushLocalAddr(-8)));
        assert!(body.contains(&Ir::PushLocalAddr(-16)));
    }

    fn test_small_types_get_correct_slot_size_but_8byte_aligned_offsets() {
        let body = lower_main("module m; fn main() { let a: u8 = 1; let b: u64 = 2; }");
        assert!(body.contains(&Ir::Store(1))); // u8 store is 1 byte...
        assert!(body.contains(&Ir::PushLocalAddr(-16))); // ...but b is still at -16, not -9
    }

    fn test_return_with_value_sets_has_value_true() {
        let body = lower_fn("module m; fn f() -> u64 { return 1; } fn main() {}", "f");
        assert!(body.contains(&Ir::Ret(true)));
    }

    fn test_return_without_value_sets_has_value_false() {
        let body = lower_fn("module m; fn f() { return; } fn main() {}", "f");
        assert_eq!(body, vec![Ir::Ret(false), Ir::Ret(false)]);
    }

    fn test_binary_op_lowers_operands_then_op() {
        let body = lower_main("module m; fn main() { let x: u64 = 1 + 2; }");
        assert_eq!(&body[0..3], &[
            Ir::PushImm(1),
            Ir::PushImm(2),
            Ir::BinOp(crate::k::ast::BinOp::Add),
        ]);
    }

    fn test_if_without_else_lowering_shape() {
        let body = lower_main("module m; fn main() { if 1 { let x: u64 = 1; } }");
        assert_eq!(body[0], Ir::PushImm(1));
        assert!(matches!(body[1], Ir::JumpIfZero(_)));
        let has_jump = body.iter().any(|op| matches!(op, Ir::Jump(_)));
        assert!(has_jump);
        let label_count = body.iter().filter(|op| matches!(op, Ir::Label(_))).count();
        assert_eq!(label_count, 2); // else_lbl and end_lbl
    }

    fn test_while_loop_has_start_and_end_labels() {
        let body = lower_main("module m; fn main() { mut i: u64 = 0; while i < 3 { i = i + 1; } }");
        let label_count = body.iter().filter(|op| matches!(op, Ir::Label(_))).count();
        assert_eq!(label_count, 2);
        assert!(body.iter().any(|op| matches!(op, Ir::JumpIfZero(_))));
        assert!(body.iter().any(|op| matches!(op, Ir::Jump(_))));
    }

    fn test_break_jumps_to_loop_end_label() {
        let body = lower_main("module m; fn main() { loop { break; } }");
        let Ir::Label(start_id) = body[0] else { panic!("expected label first") };
        let Ir::Jump(break_target) = body[1] else { panic!("expected jump for break") };
        let end_label_id = body.iter().find_map(|op| match op {
            Ir::Label(id) if *id != start_id => Some(*id),
            _ => None,
        }).unwrap();
        assert_eq!(break_target, end_label_id);
    }

    fn test_continue_jumps_to_loop_start_label() {
        let body = lower_main("module m; fn main() { loop { continue; } }");
        let Ir::Label(start_id) = body[0] else { panic!("expected label first") };
        let Ir::Jump(continue_target) = body[1] else { panic!("expected jump for continue") };
        assert_eq!(continue_target, start_id);
    }

    fn test_struct_field_write_offset() {
        let src = "module m; struct P { x: u64; y: u64; } fn main() { let p: P; p.y = 5; }";
        let body = lower_main(src);
        let imm_pos = body.iter().position(|op| matches!(op, Ir::PushImm(5))).unwrap();
        assert!(matches!(body[imm_pos + 1], Ir::PushLocalAddr(_)));
        assert_eq!(body[imm_pos + 2], Ir::OffsetAddr(8));
        assert_eq!(body[imm_pos + 3], Ir::Store(8));
    }

    fn test_array_index_write_uses_index_addr() {
        let src = "module m; fn main() { let a: u64[3] = [1,2,3]; a[1] = 9; }";
        let body = lower_main(src);
        let imm_pos = body.iter().position(|op| matches!(op, Ir::PushImm(9))).unwrap();
        assert_eq!(body[imm_pos + 3], Ir::IndexAddr(8));
        assert_eq!(body[imm_pos + 4], Ir::Store(8));
    }

    fn test_array_literal_init_stores_each_element() {
        let src = "module m; fn main() { let a: u64[3] = [10,20,30]; }";
        let body = lower_main(src);
        let stores = body.iter().filter(|op| matches!(op, Ir::Store(8))).count();
        assert_eq!(stores, 3);
        assert!(body.contains(&Ir::OffsetAddr(0)));
        assert!(body.contains(&Ir::OffsetAddr(8)));
        assert!(body.contains(&Ir::OffsetAddr(16)));
    }

    fn test_deref_read_loads_8_bytes_unsigned() {
        let src = "module m; fn main() { let v: u64 = 1; let p: u64 pointer = &v; let x: u64 = *p; }";
        let body = lower_main(src);
        assert!(body.contains(&Ir::Load(8, false)));
    }

    fn test_addr_of_does_not_load() {
        let src = "module m; fn main() { let v: u64 = 1; let p: u64 pointer = &v; }";
        let body = lower_main(src);
        let addr_of_pos = body.iter().rposition(|op| matches!(op, Ir::PushLocalAddr(-8))).unwrap();
        assert_ne!(body.get(addr_of_pos + 1), Some(&Ir::Load(8, false)));
    }

    fn test_postincr_sequence_matches_documented_shape() {
        let src = "module m; fn main() { mut x: u64 = 0; x++; }";
        let body = lower_main(src);
        let start = body.iter().position(|op| matches!(op, Ir::Load(8, false))).unwrap();
        assert_eq!(body[start], Ir::Load(8, false));
        assert!(matches!(body[start + 1], Ir::PushLocalAddr(_)));
        assert_eq!(body[start + 2], Ir::Dup);
        assert_eq!(body[start + 3], Ir::Load(8, false));
        assert_eq!(body[start + 4], Ir::PushImm(1));
        assert_eq!(body[start + 5], Ir::BinOp(crate::k::ast::BinOp::Add));
        assert_eq!(body[start + 6], Ir::Swap);
        assert_eq!(body[start + 7], Ir::Store(8));
    }

    fn test_postdecr_uses_sub() {
        let src = "module m; fn main() { mut x: u64 = 0; x--; }";
        let body = lower_main(src);
        assert!(body.contains(&Ir::BinOp(crate::k::ast::BinOp::Sub)));
    }

    fn test_match_lowers_to_equality_chain() {
        let src = "module m; enum S { A, B, C } fn main() { let s: S = S.B; \
                    match s { S.A => { } S.B => { } S.C => { } } }";
        let body = lower_main(src);
        let eq_count = body.iter().filter(|op| matches!(op, Ir::BinOp(crate::k::ast::BinOp::Eq))).count();
        assert_eq!(eq_count, 3);
        assert!(body.contains(&Ir::PushImm(0)));
        assert!(body.contains(&Ir::PushImm(1)));
        assert!(body.contains(&Ir::PushImm(2)));
    }

    fn test_kernel_print_literal_expands_to_write_bytes() {
        let body = lower_main(r#"module m; fn main() { kernel.print("hi!"); }"#);
        let write_bytes_num = KERNEL_SYSCALLS.iter().find(|(n, _)| *n == "write_bytes").unwrap().1;
        assert!(body.iter().any(|op| matches!(op, Ir::PushGlobalAddr(_))));
        assert!(body.contains(&Ir::PushImm(3))); // "hi!".len() == 3
        assert!(body.contains(&Ir::KernelCall(write_bytes_num, 2)));
    }

    fn test_kernel_exit_uses_correct_syscall_number() {
        let body = lower_main("module m; fn main() { kernel.exit(0); }");
        let exit_num = KERNEL_SYSCALLS.iter().find(|(n, _)| *n == "exit").unwrap().1;
        assert!(body.contains(&Ir::KernelCall(exit_num, 1)));
    }

    fn test_function_call_lowering() {
        let src = "module m; fn add(a: u64, b: u64) -> u64 { return a + b; } \
                    fn main() { let x: u64 = add(1, 2); }";
        let body = lower_main(src);
        assert_eq!(&body[0..3], &[
            Ir::PushImm(1),
            Ir::PushImm(2),
            Ir::Call("add".to_string(), 2),
        ]);
    }

    fn test_sizeof_is_compile_time_constant() {
        let body = lower_main("module m; fn main() { let x: u64 = sizeof(u64); }");
        assert_eq!(body[0], Ir::PushImm(8));
    }

    fn test_sizeof_struct() {
        let src = "module m; struct P { x: u64; y: u64; } fn main() { let s: u64 = sizeof(P); }";
        let body = lower_main(src);
        assert_eq!(body[0], Ir::PushImm(16));
    }

    fn test_frame_size_16byte_aligned() {
        let m = parse("module m; fn main() { let a: u64 = 1; }").unwrap();
        let sema = analyze(&m).unwrap();
        let prog = lower(&m, &sema);
        let f = prog.functions.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(f.frame_size % 16, 0);
        assert!(f.frame_size >= 8);
    }

    fn test_param_offsets_recorded_for_each_param() {
        let m = parse("module m; fn f(a: u64, b: u64) {} fn main() {}").unwrap();
        let sema = analyze(&m).unwrap();
        let prog = lower(&m, &sema);
        let f = prog.functions.iter().find(|f| f.name == "f").unwrap();
        assert_eq!(f.param_offsets.len(), 2);
        assert_ne!(f.param_offsets[0], f.param_offsets[1]);
    }

    fn test_string_interning_produces_unique_labels() {
        let m = parse(r#"module m; fn main() { kernel.print("a"); kernel.print("b"); }"#).unwrap();
        let sema = analyze(&m).unwrap();
        let prog = lower(&m, &sema);
        assert_eq!(prog.strings.len(), 2);
        assert_ne!(prog.strings[0].0, prog.strings[1].0);
    }

    fn test_bitwise_and_shift_ops_lower() {
        let body = lower_main("module m; fn main() { let x: u64 = (1 & 2) | (3 ^ 4); let y: u64 = 1 << 2; let z: u64 = 8 >> 1; }");
        use crate::k::ast::BinOp::*;
        assert!(body.contains(&Ir::BinOp(BitAnd)));
        assert!(body.contains(&Ir::BinOp(BitOr)));
        assert!(body.contains(&Ir::BinOp(BitXor)));
        assert!(body.contains(&Ir::BinOp(Shl)));
        assert!(body.contains(&Ir::BinOp(Shr)));
    }

    fn test_unary_ops_lower() {
        let body = lower_main("module m; fn main() { let a: i64 = -1; let b: bool = !true; let c: u64 = ~1; }");
        assert!(body.contains(&Ir::Neg));
        assert!(body.contains(&Ir::Not));
        assert!(body.contains(&Ir::BitNot));
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_empty_function_just_returns,
            &test_let_with_init_pushes_value_then_addr_then_stores,
            &test_let_without_init_emits_nothing,
            &test_assign_pushes_value_then_addr_then_stores,
            &test_locals_get_sequential_negative_offsets,
            &test_small_types_get_correct_slot_size_but_8byte_aligned_offsets,
            &test_return_with_value_sets_has_value_true,
            &test_return_without_value_sets_has_value_false,
            &test_binary_op_lowers_operands_then_op,
            &test_if_without_else_lowering_shape,
            &test_while_loop_has_start_and_end_labels,
            &test_break_jumps_to_loop_end_label,
            &test_continue_jumps_to_loop_start_label,
            &test_struct_field_write_offset,
            &test_array_index_write_uses_index_addr,
            &test_array_literal_init_stores_each_element,
            &test_deref_read_loads_8_bytes_unsigned,
            &test_addr_of_does_not_load,
            &test_postincr_sequence_matches_documented_shape,
            &test_postdecr_uses_sub,
            &test_match_lowers_to_equality_chain,
            &test_kernel_print_literal_expands_to_write_bytes,
            &test_kernel_exit_uses_correct_syscall_number,
            &test_function_call_lowering,
            &test_sizeof_is_compile_time_constant,
            &test_sizeof_struct,
            &test_frame_size_16byte_aligned,
            &test_param_offsets_recorded_for_each_param,
            &test_string_interning_produces_unique_labels,
            &test_bitwise_and_shift_ops_lower,
            &test_unary_ops_lower,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("OK");
    }
}
