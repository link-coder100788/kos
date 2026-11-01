//! typecheck.rs — builds symbol tables (structs, enums, functions) and performs
//! lightweight semantic checks (undefined names, mutability, arity).

use super::ast::*;
use hashbrown::HashMap;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use alloc::vec;
use alloc::string::ToString;

#[derive(Debug, Clone)]
pub struct StructInfo {
    pub fields: Vec<(String, Type, u64 /* offset */)>,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub variants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<(String, Type)>,
    pub ret: Type,
}

#[derive(Debug)]
pub struct Sema {
    pub structs: HashMap<String, StructInfo>,
    pub enums: HashMap<String, EnumInfo>,
    pub functions: HashMap<String, FnSig>,
}

// Mirrors ir::KERNEL_SYSCALLS and ir::KERNEL_SUBCALLS, plus `print` — which isn't a real syscall;
// it's a compiler convenience that only accepts a string literal and
// expands to `write_bytes(ptr, compile_time_known_len)` (see ir.rs).
const KERNEL_FNS: &[(&str, usize)] = &[
    ("exit", 1), ("write_byte", 1), ("write_bytes", 2), ("read_line", 2), ("print", 1), ("dump", 0),
    ("sleep_ms", 1), ("snd", 1), ("beep", 0), ("play", 1), ("play_file", 1),
];

pub fn field_size(sema: &Sema, ty: &Type) -> u64 {
    match ty {
        Type::Named(name) => sema.structs.get(name).map(|s| s.size).unwrap_or(8),
        Type::Array(t, n) => field_size(sema, t) * n,
        other => other.size(),
    }
}

pub fn analyze(module: &Module) -> Result<Sema, String> {
    let mut structs = HashMap::new();
    let mut enums = HashMap::new();
    let mut functions = HashMap::new();

    // Pass 1: collect struct/enum names so field types referencing other
    // structs can be resolved (only need sizes so simple forward pass + fixups works
    // as long as there's no circularity — reasonable for this language).
    for item in &module.items {
        match item {
            Item::Struct(s) => {
                if structs.contains_key(&s.name) {
                    return Err(format!("duplicate struct '{}'", s.name));
                }
                structs.insert(s.name.clone(), StructInfo { fields: vec![], size: 0 });
            }
            Item::Enum(e) => {
                if enums.contains_key(&e.name) {
                    return Err(format!("duplicate enum '{}'", e.name));
                }
                enums.insert(e.name.clone(), EnumInfo { variants: e.variants.clone() });
            }
            Item::Function(_) => {}
        }
    }

    // Pass 2: compute struct layouts (naive: sequential offsets, no padding —
    // matches K's "simple memory model" philosophy).
    for item in &module.items {
        if let Item::Struct(s) = item {
            let mut fields = Vec::new();
            let mut offset = 0u64;
            for (fname, fty) in &s.fields {
                let sz = match fty {
                    Type::Named(n) => structs.get(n).map(|si| si.size).unwrap_or(8),
                    other => other.size(),
                };
                fields.push((fname.clone(), fty.clone(), offset));
                offset += sz.max(1);
            }
            structs.insert(s.name.clone(), StructInfo { fields, size: offset });
        }
    }

    // Pass 3: function signatures.
    for item in &module.items {
        if let Item::Function(f) = item {
            if functions.contains_key(&f.name) {
                return Err(format!("duplicate function '{}'", f.name));
            }
            functions.insert(f.name.clone(), FnSig { params: f.params.clone(), ret: f.ret.clone() });
        }
    }

    if !functions.contains_key("main") {
        return Err("no 'main' function found (K requires a program entry point)".to_string());
    }

    let sema = Sema { structs, enums, functions };

    // Pass 4: check each function body.
    for item in &module.items {
        if let Item::Function(f) = item {
            check_function(&sema, f)?;
        }
    }

    Ok(sema)
}

struct Scope {
    vars: Vec<HashMap<String, (Type, bool /* mutable */)>>,
}
impl Scope {
    fn new() -> Self { Scope { vars: vec![HashMap::new()] } }
    fn push(&mut self) { self.vars.push(HashMap::new()); }
    fn pop(&mut self) { self.vars.pop(); }
    fn define(&mut self, name: &str, ty: Type, mutable: bool) {
        self.vars.last_mut().unwrap().insert(name.to_string(), (ty, mutable));
    }
    fn lookup(&self, name: &str) -> Option<&(Type, bool)> {
        for scope in self.vars.iter().rev() {
            if let Some(v) = scope.get(name) { return Some(v); }
        }
        None
    }
}

fn check_function(sema: &Sema, f: &Function) -> Result<(), String> {
    let mut scope = Scope::new();
    for (pname, pty) in &f.params {
        scope.define(pname, pty.clone(), true);
    }
    check_block(sema, &f.body, &mut scope, false)?;
    Ok(())
}

fn check_block(sema: &Sema, blk: &Block, scope: &mut Scope, in_loop: bool) -> Result<(), String> {
    scope.push();
    for stmt in blk {
        check_stmt(sema, stmt, scope, in_loop)?;
    }
    scope.pop();
    Ok(())
}

fn check_stmt(sema: &Sema, stmt: &Stmt, scope: &mut Scope, in_loop: bool) -> Result<(), String> {
    match stmt {
        Stmt::Let { name, mutable, ty, init } => {
            if let Some(e) = init { check_expr(sema, e, scope)?; }
            scope.define(name, ty.clone(), *mutable);
        }
        Stmt::Assign { target, value } => {
            check_lvalue_mutable(sema, target, scope)?;
            check_expr(sema, value, scope)?;
        }
        Stmt::ExprStmt(e) => { check_expr(sema, e, scope)?; }
        Stmt::Return(e) => { if let Some(e) = e { check_expr(sema, e, scope)?; } }
        Stmt::If { cond, then_blk, else_blk } => {
            check_expr(sema, cond, scope)?;
            check_block(sema, then_blk, scope, in_loop)?;
            if let Some(eb) = else_blk { check_block(sema, eb, scope, in_loop)?; }
        }
        Stmt::While { cond, body } => {
            check_expr(sema, cond, scope)?;
            check_block(sema, body, scope, true)?;
        }
        Stmt::Loop { body } => { check_block(sema, body, scope, true)?; }
        Stmt::Break => { if !in_loop { return Err("break outside of loop".to_string()); } }
        Stmt::Continue => { if !in_loop { return Err("continue outside of loop".to_string()); } }
        Stmt::Match { scrutinee, arms } => {
            check_expr(sema, scrutinee, scope)?;
            for (_variant, blk) in arms {
                check_block(sema, blk, scope, in_loop)?;
            }
        }
    }
    Ok(())
}

fn check_lvalue_mutable(sema: &Sema, e: &Expr, scope: &Scope) -> Result<(), String> {
    match e {
        Expr::Ident(name) => {
            match scope.lookup(name) {
                Some((_, true)) => Ok(()),
                Some((_, false)) => Err(format!("cannot assign to immutable variable '{}'", name)),
                None => Err(format!("undefined variable '{}'", name)),
            }
        }
        Expr::Deref(inner) => { check_expr(sema, inner, scope) }
        Expr::Index(base, idx) => { check_expr(sema, base, scope)?; check_expr(sema, idx, scope) }
        Expr::Field(base, _) => { check_expr(sema, base, scope) }
        Expr::ArrowField(base, _) => { check_expr(sema, base, scope) }
        _ => Err("invalid assignment target".to_string()),
    }
}

fn check_expr(sema: &Sema, e: &Expr, scope: &Scope) -> Result<(), String> {
    match e {
        Expr::IntLit(_) | Expr::BoolLit(_) | Expr::StrLit(_) | Expr::CharLit(_) | Expr::Sizeof(_) => Ok(()),
        Expr::Ident(name) => {
            if scope.lookup(name).is_some() { Ok(()) }
            else { Err(format!("undefined variable '{}'", name)) }
        }
        Expr::ArrayLit(items) => { for it in items { check_expr(sema, it, scope)?; } Ok(()) }
        Expr::Index(b, i) => { check_expr(sema, b, scope)?; check_expr(sema, i, scope) }
        Expr::Field(b, _) => check_expr(sema, b, scope),
        Expr::ArrowField(b, _) => check_expr(sema, b, scope),
        Expr::Unary(_, inner) => check_expr(sema, inner, scope),
        Expr::Binary(_, l, r) => { check_expr(sema, l, scope)?; check_expr(sema, r, scope) }
        Expr::Call(name, args) => {
            let sig = sema.functions.get(name).ok_or(format!("undefined function '{}'", name))?;
            if sig.params.len() != args.len() {
                return Err(format!("function '{}' expects {} args, got {}", name, sig.params.len(), args.len()));
            }
            for a in args { check_expr(sema, a, scope)?; }
            Ok(())
        }
        Expr::KernelCall(name, args) => {
            let arity = KERNEL_FNS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
                .ok_or(format!("unknown kernel function 'kernel.{}'", name))?;
            if arity != args.len() {
                return Err(format!("kernel.{} expects {} args, got {}", name, arity, args.len()));
            }
            // print is expanded at compile time using the literal's length.
            if name == "print" && !matches!(args[0], Expr::StrLit(_)) {
                return Err(String::from("kernel.print() only supports string literal arguments; use kernel.write_bytes(ptr, len) for anything else"));
            }
            if (name == "play" || name == "play_file") && !matches!(args[0], Expr::StrLit(_)) {
                return Err(format!("kernel.{}() only supports a string literal, e.g. kernel.{}(\"...\")", name, name));
            }
            for a in args { check_expr(sema, a, scope)?; }
            Ok(())
        }
        Expr::AddrOf(inner) => check_expr(sema, inner, scope),
        Expr::Deref(inner) => check_expr(sema, inner, scope),
        Expr::EnumVariant(ename, variant) => {
            let info = sema.enums.get(ename).ok_or(format!("undefined enum '{}'", ename))?;
            if !info.variants.contains(variant) {
                return Err(format!("enum '{}' has no variant '{}'", ename, variant));
            }
            Ok(())
        }
        Expr::PostIncr(inner) | Expr::PostDecr(inner) => check_lvalue_mutable(sema, inner, scope),
    }
}

//#[cfg(feature = "test")]
pub mod tests {
    use super::*;
    use crate::k::parser::parse;
    use alloc::boxed::Box;
    use crate::println;

    fn ok(src: &str) -> Sema {
        let m = parse(src).expect("parse should succeed");
        analyze(&m).unwrap_or_else(|e| panic!("expected analyze success, got: {}", e))
    }
    fn err(src: &str) -> String {
        let m = parse(src).expect("parse should succeed");
        analyze(&m).expect_err("expected analyze error")
    }

    fn test_minimal_valid_program() {
        ok("module m; fn main() {}");
    }

    fn test_missing_main_errors() {
        let e = err("module m; fn other() {}");
        assert!(e.contains("main"));
    }

    fn test_duplicate_function_errors() {
        let e = err("module m; fn f() {} fn f() {} fn main() {}");
        assert!(e.contains("duplicate function"));
    }

    fn test_duplicate_struct_errors() {
        let e = err("module m; struct P { x: u64; } struct P { y: u64; } fn main() {}");
        assert!(e.contains("duplicate struct"));
    }

    fn test_duplicate_enum_errors() {
        let e = err("module m; enum E { A } enum E { B } fn main() {}");
        assert!(e.contains("duplicate enum"));
    }

    fn test_struct_layout_sequential_offsets() {
        let sema = ok("module m; struct P { x: u64; y: u32; z: u8; } fn main() {}");
        let p = &sema.structs["P"];
        assert_eq!(p.fields[0], ("x".to_string(), Type::U64, 0));
        assert_eq!(p.fields[1], ("y".to_string(), Type::U32, 8));
        assert_eq!(p.fields[2], ("z".to_string(), Type::U8, 12));
        assert_eq!(p.size, 13);
    }

    fn test_nested_struct_layout() {
        let sema = ok("module m; struct Inner { a: u64; } struct Outer { i: Inner; b: u8; } fn main() {}");
        let outer = &sema.structs["Outer"];
        assert_eq!(outer.fields[0].2, 0); // Inner at offset 0
        assert_eq!(outer.fields[1].2, 8); // b at offset 8 (after 8-byte Inner)
        assert_eq!(outer.size, 9);
    }

    fn test_enum_variants_recorded() {
        let sema = ok("module m; enum S { Ready, Running, Dead } fn main() {}");
        assert_eq!(sema.enums["S"].variants, vec!["Ready", "Running", "Dead"]);
    }

    fn test_function_signature_recorded() {
        let sema = ok("module m; fn add(a: u64, b: u64) -> u64 { return a + b; } fn main() {}");
        let sig = &sema.functions["add"];
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.ret, Type::U64);
    }

    fn test_undefined_variable_errors() {
        let e = err("module m; fn main() { let x: u64 = y; }");
        assert!(e.contains("undefined variable"));
    }

    fn test_assign_to_immutable_errors() {
        let e = err("module m; fn main() { let x: u64 = 1; x = 2; }");
        assert!(e.contains("immutable"));
    }

    fn test_assign_to_mutable_ok() {
        ok("module m; fn main() { mut x: u64 = 1; x = 2; }");
    }

    fn test_assign_to_undefined_errors() {
        let e = err("module m; fn main() { x = 2; }");
        assert!(e.contains("undefined variable"));
    }

    fn test_break_outside_loop_errors() {
        let e = err("module m; fn main() { break; }");
        assert!(e.contains("break"));
    }

    fn test_continue_outside_loop_errors() {
        let e = err("module m; fn main() { continue; }");
        assert!(e.contains("continue"));
    }

    fn test_break_inside_while_ok() {
        ok("module m; fn main() { while 1 { break; } }");
    }

    fn test_break_inside_loop_ok() {
        ok("module m; fn main() { loop { break; } }");
    }

    fn test_break_inside_nested_block_in_loop_ok() {
        ok("module m; fn main() { loop { if 1 { break; } } }");
    }

    fn test_call_undefined_function_errors() {
        let e = err("module m; fn main() { foo(); }");
        assert!(e.contains("undefined function"));
    }

    fn test_call_wrong_arity_errors() {
        let e = err("module m; fn f(a: u64) {} fn main() { f(1, 2); }");
        assert!(e.contains("expects"));
    }

    fn test_call_correct_arity_ok() {
        ok("module m; fn f(a: u64, b: u64) {} fn main() { f(1, 2); }");
    }

    fn test_kernel_call_unknown_errors() {
        let e = err("module m; fn main() { kernel.frobnicate(); }");
        assert!(e.contains("unknown kernel function"));
    }

    fn test_kernel_call_known_arity_ok() {
        ok(r#"module m; fn main() { kernel.print("hi"); kernel.exit(0); }"#);
    }

    fn test_kernel_print_non_literal_errors() {
        let e = err(r#"module m; fn main() { let s: string = "x"; kernel.print(s); }"#);
        assert!(e.contains("string literal"));
    }

    fn test_kernel_call_wrong_arity_errors() {
        let e = err("module m; fn main() { kernel.exit(); }");
        assert!(e.contains("expects"));
    }

    fn test_enum_variant_valid_ok() {
        ok("module m; enum S { A, B } fn main() { let s: S = S.A; }");
    }

    fn test_enum_variant_undefined_enum_errors() {
        let e = err("module m; fn main() { let s: u64 = Bogus.A; }");
        assert!(e.contains("undefined enum"));
    }

    fn test_enum_variant_undefined_variant_errors() {
        let e = err("module m; enum S { A, B } fn main() { let s: S = S.C; }");
        assert!(e.contains("no variant"));
    }

    fn test_scoping_shadowing_across_blocks_ok() {
        ok("module m; fn main() { if 1 { let x: u64 = 1; } if 1 { let x: u64 = 2; } }");
    }

    fn test_variable_out_of_scope_after_block_errors() {
        let e = err("module m; fn main() { if 1 { let x: u64 = 1; } let y: u64 = x; }");
        assert!(e.contains("undefined variable"));
    }

    fn test_field_size_lookup_for_named_struct() {
        let sema = ok("module m; struct P { x: u64; y: u64; } fn main() {}");
        assert_eq!(field_size(&sema, &Type::Named("P".to_string())), 16);
    }

    fn test_field_size_for_array() {
        let sema = ok("module m; fn main() {}");
        assert_eq!(field_size(&sema, &Type::Array(Box::new(Type::U32), 5)), 20);
    }

    fn test_field_size_for_primitives() {
        let sema = ok("module m; fn main() {}");
        assert_eq!(field_size(&sema, &Type::U8), 1);
        assert_eq!(field_size(&sema, &Type::U64), 8);
        assert_eq!(field_size(&sema, &Type::Pointer(Box::new(Type::U8))), 8);
    }

    fn test_pointer_and_array_and_struct_full_check_ok() {
        let src = r#"
            module m;
            struct Point { x: u64; y: u64; }
            fn dist(p: Point pointer) -> u64 { return p->x + p->y; }
            fn main() {
                let p: Point;
                p.x = 1;
                p.y = 2;
                let addr: Point pointer = &p;
                let d: u64 = dist(addr);
                let arr: u64[3] = [1, 2, 3];
                let v: u64 = arr[0];
                let val: u64 = 41;
                let ptr: u64 pointer = &val;
                *ptr = *ptr + 1;
            }
        "#;
        ok(src);
    }

    fn test_match_arms_checked() {
        let src = "module m; enum S { A, B } fn main() { let s: S = S.A; \
                    match s { S.A => { let x: u64 = 1; } S.B => { } } }";
        ok(src);
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_minimal_valid_program,
            &test_missing_main_errors,
            &test_duplicate_function_errors,
            &test_duplicate_struct_errors,
            &test_duplicate_enum_errors,
            &test_struct_layout_sequential_offsets,
            &test_nested_struct_layout,
            &test_enum_variants_recorded,
            &test_function_signature_recorded,
            &test_undefined_variable_errors,
            &test_assign_to_immutable_errors,
            &test_assign_to_mutable_ok,
            &test_assign_to_undefined_errors,
            &test_break_outside_loop_errors,
            &test_continue_outside_loop_errors,
            &test_break_inside_while_ok,
            &test_break_inside_loop_ok,
            &test_break_inside_nested_block_in_loop_ok,
            &test_call_undefined_function_errors,
            &test_call_wrong_arity_errors,
            &test_call_correct_arity_ok,
            &test_kernel_call_unknown_errors,
            &test_kernel_call_known_arity_ok,
            &test_kernel_print_non_literal_errors,
            &test_kernel_call_wrong_arity_errors,
            &test_enum_variant_valid_ok,
            &test_enum_variant_undefined_enum_errors,
            &test_enum_variant_undefined_variant_errors,
            &test_scoping_shadowing_across_blocks_ok,
            &test_variable_out_of_scope_after_block_errors,
            &test_field_size_lookup_for_named_struct,
            &test_field_size_for_array,
            &test_field_size_for_primitives,
            &test_pointer_and_array_and_struct_full_check_ok,
            &test_match_arms_checked,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("OK");
    }
}
