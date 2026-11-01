//! parser.rs — Recursive-descent parser: tokens -> AST

use super::ast::*;
use super::lexer::Tok;
use alloc::vec::Vec;
use alloc::format;
use alloc::string::String;
use alloc::boxed::Box;
use alloc::vec;

pub struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

type PResult<T> = Result<T, String>;

impl Parser {
    pub fn new(toks: Vec<(Tok, usize)>) -> Self {
        Parser { toks, pos: 0 }
    }

    fn cur(&self) -> &Tok { &self.toks[self.pos].0 }
    fn line(&self) -> usize { self.toks[self.pos].1 }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos < self.toks.len() - 1 { self.pos += 1; }
        t
    }
    fn check(&self, t: &Tok) -> bool { self.cur() == t }
    fn eat(&mut self, t: &Tok) -> PResult<()> {
        if self.check(t) { self.bump(); Ok(()) }
        else { Err(format!("line {}: expected {:?}, found {:?}", self.line(), t, self.cur())) }
    }
    fn ident(&mut self) -> PResult<String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            other => Err(format!("line {}: expected identifier, found {:?}", self.line(), other)),
        }
    }

    pub fn parse_module(&mut self) -> PResult<Module> {
        self.eat(&Tok::Module)?;
        let name = self.ident()?;
        self.eat(&Tok::Semi)?;

        let mut imports = Vec::new();
        while self.check(&Tok::Import) {
            self.bump();
            let name = self.ident()?;
            self.eat(&Tok::Semi)?;
            imports.push(name);
        }

        let mut items = Vec::new();
        while !self.check(&Tok::Eof) {
            items.push(self.parse_item()?);
        }

        Ok(Module { name, imports, items })
    }

    fn parse_item(&mut self) -> PResult<Item> {
        match self.cur() {
            Tok::Fn => Ok(Item::Function(self.parse_function()?)),
            Tok::Struct => Ok(Item::Struct(self.parse_struct()?)),
            Tok::Enum => Ok(Item::Enum(self.parse_enum()?)),
            other => Err(format!("line {}: expected item (fn/struct/enum), found {:?}", self.line(), other)),
        }
    }

    fn parse_type(&mut self) -> PResult<Type> {
        let mut ty = match self.bump() {
            Tok::Ident(name) => match name.as_str() {
                "u8" => Type::U8, "u16" => Type::U16, "u32" => Type::U32, "u64" => Type::U64,
                "i8" => Type::I8, "i16" => Type::I16, "i32" => Type::I32, "i64" => Type::I64,
                "usize" => Type::Usize, "isize" => Type::Isize,
                "bool" => Type::Bool, "char" => Type::Char, "string" => Type::Str, "void" => Type::Void,
                _ => Type::Named(name),
            },
            other => return Err(format!("line {}: expected type, found {:?}", self.line(), other)),
        };
        loop {
            if self.check(&Tok::Pointer) {
                self.bump();
                ty = Type::Pointer(Box::new(ty));
            } else if self.check(&Tok::LBracket) {
                self.bump();
                let n = match self.bump() {
                    Tok::Int(n) => n,
                    other => return Err(format!("line {}: expected array size, found {:?}", self.line(), other)),
                };
                self.eat(&Tok::RBracket)?;
                ty = Type::Array(Box::new(ty), n);
            } else {
                break;
            }
        }
        Ok(ty)
    }

    fn parse_function(&mut self) -> PResult<Function> {
        self.eat(&Tok::Fn)?;
        let name = self.ident()?;
        self.eat(&Tok::LParen)?;
        let mut params = Vec::new();
        while !self.check(&Tok::RParen) {
            let pname = self.ident()?;
            self.eat(&Tok::Colon)?;
            let pty = self.parse_type()?;
            params.push((pname, pty));
            if self.check(&Tok::Comma) { self.bump(); } else { break; }
        }
        self.eat(&Tok::RParen)?;
        let ret = if self.check(&Tok::Arrow) {
            self.bump();
            self.parse_type()?
        } else {
            Type::Void
        };
        let body = self.parse_block()?;
        Ok(Function { name, params, ret, body })
    }

    fn parse_struct(&mut self) -> PResult<StructDef> {
        self.eat(&Tok::Struct)?;
        let name = self.ident()?;
        self.eat(&Tok::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&Tok::RBrace) {
            let fname = self.ident()?;
            self.eat(&Tok::Colon)?;
            let fty = self.parse_type()?;
            self.eat(&Tok::Semi)?;
            fields.push((fname, fty));
        }
        self.eat(&Tok::RBrace)?;
        Ok(StructDef { name, fields })
    }

    fn parse_enum(&mut self) -> PResult<EnumDef> {
        self.eat(&Tok::Enum)?;
        let name = self.ident()?;
        self.eat(&Tok::LBrace)?;
        let mut variants = Vec::new();
        while !self.check(&Tok::RBrace) {
            variants.push(self.ident()?);
            if self.check(&Tok::Comma) { self.bump(); } else { break; }
        }
        self.eat(&Tok::RBrace)?;
        Ok(EnumDef { name, variants })
    }

    fn parse_block(&mut self) -> PResult<Block> {
        self.eat(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&Tok::RBrace) {
            stmts.push(self.parse_stmt()?);
        }
        self.eat(&Tok::RBrace)?;
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        match self.cur() {
            Tok::Let => {
                self.bump();
                let name = self.ident()?;
                self.eat(&Tok::Colon)?;
                let ty = self.parse_type()?;
                let init = if self.check(&Tok::Eq) {
                    self.bump();
                    Some(self.parse_expr()?)
                } else { None };
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Let { name, mutable: false, ty, init })
            }
            Tok::Mut => {
                self.bump();
                let name = self.ident()?;
                self.eat(&Tok::Colon)?;
                let ty = self.parse_type()?;
                let init = if self.check(&Tok::Eq) {
                    self.bump();
                    Some(self.parse_expr()?)
                } else { None };
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Let { name, mutable: true, ty, init })
            }
            Tok::Return => {
                self.bump();
                if self.check(&Tok::Semi) {
                    self.bump();
                    Ok(Stmt::Return(None))
                } else {
                    let e = self.parse_expr()?;
                    self.eat(&Tok::Semi)?;
                    Ok(Stmt::Return(Some(e)))
                }
            }
            Tok::If => Ok(self.parse_if()?),
            Tok::While => {
                self.bump();
                let cond = self.parse_expr()?;
                let body = self.parse_block()?;
                Ok(Stmt::While { cond, body })
            }
            Tok::Loop => {
                self.bump();
                let body = self.parse_block()?;
                Ok(Stmt::Loop { body })
            }
            Tok::Break => { self.bump(); self.eat(&Tok::Semi)?; Ok(Stmt::Break) }
            Tok::Continue => { self.bump(); self.eat(&Tok::Semi)?; Ok(Stmt::Continue) }
            Tok::Match => self.parse_match(),
            _ => {
                // expr statement, assignment, postfix incr/decr
                let e = self.parse_expr()?;
                if self.check(&Tok::Eq) {
                    self.bump();
                    let value = self.parse_expr()?;
                    self.eat(&Tok::Semi)?;
                    Ok(Stmt::Assign { target: e, value })
                } else {
                    self.eat(&Tok::Semi)?;
                    Ok(Stmt::ExprStmt(e))
                }
            }
        }
    }

    fn parse_if(&mut self) -> PResult<Stmt> {
        self.eat(&Tok::If)?;
        let cond = self.parse_expr()?;
        let then_blk = self.parse_block()?;
        let else_blk = if self.check(&Tok::Else) {
            self.bump();
            if self.check(&Tok::If) {
                Some(vec![self.parse_if()?])
            } else {
                Some(self.parse_block()?)
            }
        } else { None };
        Ok(Stmt::If { cond, then_blk, else_blk })
    }

    fn parse_match(&mut self) -> PResult<Stmt> {
        self.eat(&Tok::Match)?;
        let scrutinee = self.parse_expr()?;
        self.eat(&Tok::LBrace)?;
        let mut arms = Vec::new();
        while !self.check(&Tok::RBrace) {
            // EnumName.Variant => { block }
            let _enum_name = self.ident()?;
            self.eat(&Tok::Dot)?;
            let variant = self.ident()?;
            self.eat(&Tok::FatArrow)?;
            let blk = self.parse_block()?;
            arms.push((variant, blk));
            if self.check(&Tok::Comma) { self.bump(); }
        }
        self.eat(&Tok::RBrace)?;
        Ok(Stmt::Match { scrutinee, arms })
    }

    // ---- expressions (precedence climbing) ----

    fn parse_expr(&mut self) -> PResult<Expr> { self.parse_or() }

    fn parse_or(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_and()?;
        while self.check(&Tok::OrOr) {
            self.bump();
            let rhs = self.parse_and()?;
            lhs = Expr::Binary(BinOp::Or, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_and(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_bitor()?;
        while self.check(&Tok::AndAnd) {
            self.bump();
            let rhs = self.parse_bitor()?;
            lhs = Expr::Binary(BinOp::And, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_bitor(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_bitxor()?;
        while self.check(&Tok::Pipe) {
            self.bump();
            let rhs = self.parse_bitxor()?;
            lhs = Expr::Binary(BinOp::BitOr, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_bitxor(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_bitand()?;
        while self.check(&Tok::Caret) {
            self.bump();
            let rhs = self.parse_bitand()?;
            lhs = Expr::Binary(BinOp::BitXor, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_bitand(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_eq()?;
        while self.check(&Tok::Amp) {
            self.bump();
            let rhs = self.parse_eq()?;
            lhs = Expr::Binary(BinOp::BitAnd, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_eq(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_rel()?;
        loop {
            let op = match self.cur() {
                Tok::EqEq => BinOp::Eq,
                Tok::NotEq => BinOp::Ne,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_rel()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_rel(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_shift()?;
        loop {
            let op = match self.cur() {
                Tok::Lt => BinOp::Lt, Tok::Gt => BinOp::Gt,
                Tok::Le => BinOp::Le, Tok::Ge => BinOp::Ge,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_shift()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_shift(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_add()?;
        loop {
            let op = match self.cur() {
                Tok::Shl => BinOp::Shl, Tok::Shr => BinOp::Shr,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_add()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_add(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.cur() {
                Tok::Plus => BinOp::Add, Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn parse_mul(&mut self) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.cur() {
                Tok::Star => BinOp::Mul, Tok::Slash => BinOp::Div, Tok::Percent => BinOp::Mod,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        match self.cur() {
            Tok::Minus => { self.bump(); Ok(Expr::Unary(UnOp::Neg, Box::new(self.parse_unary()?))) }
            Tok::Not => { self.bump(); Ok(Expr::Unary(UnOp::Not, Box::new(self.parse_unary()?))) }
            Tok::Tilde => { self.bump(); Ok(Expr::Unary(UnOp::BitNot, Box::new(self.parse_unary()?))) }
            Tok::Amp => { self.bump(); Ok(Expr::AddrOf(Box::new(self.parse_unary()?))) }
            Tok::Star => { self.bump(); Ok(Expr::Deref(Box::new(self.parse_unary()?))) }
            Tok::Sizeof => {
                self.bump();
                self.eat(&Tok::LParen)?;
                let ty = self.parse_type()?;
                self.eat(&Tok::RParen)?;
                Ok(Expr::Sizeof(ty))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            match self.cur() {
                Tok::LBracket => {
                    self.bump();
                    let idx = self.parse_expr()?;
                    self.eat(&Tok::RBracket)?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
                }
                Tok::Dot => {
                    self.bump();
                    let name = self.ident()?;
                    e = Expr::Field(Box::new(e), name);
                }
                Tok::Arrow => {
                    self.bump();
                    let name = self.ident()?;
                    e = Expr::ArrowField(Box::new(e), name);
                }
                Tok::PlusPlus => { self.bump(); e = Expr::PostIncr(Box::new(e)); }
                Tok::MinusMinus => { self.bump(); e = Expr::PostDecr(Box::new(e)); }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        match self.bump() {
            Tok::Int(n) => Ok(Expr::IntLit(n)),
            Tok::True => Ok(Expr::BoolLit(true)),
            Tok::False => Ok(Expr::BoolLit(false)),
            Tok::Str(s) => Ok(Expr::StrLit(s)),
            Tok::Char(c) => Ok(Expr::CharLit(c)),
            Tok::LParen => {
                let e = self.parse_expr()?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            Tok::LBracket => {
                let mut items = Vec::new();
                while !self.check(&Tok::RBracket) {
                    items.push(self.parse_expr()?);
                    if self.check(&Tok::Comma) { self.bump(); } else { break; }
                }
                self.eat(&Tok::RBracket)?;
                Ok(Expr::ArrayLit(items))
            }
            Tok::Ident(name) => {
                if name == "kernel" && self.check(&Tok::Dot) {
                    self.bump();
                    let fname = self.ident()?;
                    self.eat(&Tok::LParen)?;
                    let args = self.parse_args()?;
                    return Ok(Expr::KernelCall(fname, args));
                }
                if self.check(&Tok::LParen) {
                    self.bump();
                    let args = self.parse_args()?;
                    return Ok(Expr::Call(name, args));
                }
                // possible EnumName.Variant — disambiguated at typecheck; only
                // treat as enum-variant syntax if next is Dot followed by ident
                // and not followed by '(' (that would be a method-less field, still fine)
                if self.check(&Tok::Dot) {
                    // lookahead: Dot Ident, and NOT part of a longer field chain used as lvalue struct field
                    // We can't fully disambiguate without symbol table; parser treats `Name.Ident`
                    // as EnumVariant only when Name starts with uppercase (K convention), else Field.
                    if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                        self.bump(); // consume dot
                        let variant = self.ident()?;
                        return Ok(Expr::EnumVariant(name, variant));
                    }
                }
                Ok(Expr::Ident(name))
            }
            other => Err(format!("line {}: unexpected token in expression: {:?}", self.line(), other)),
        }
    }

    fn parse_args(&mut self) -> PResult<Vec<Expr>> {
        let mut args = Vec::new();
        while !self.check(&Tok::RParen) {
            args.push(self.parse_expr()?);
            if self.check(&Tok::Comma) { self.bump(); } else { break; }
        }
        self.eat(&Tok::RParen)?;
        Ok(args)
    }
}

pub fn parse(src: &str) -> PResult<Module> {
    let toks = super::lexer::Lexer::new(src).tokenize()?;
    let mut p = Parser::new(toks);
    p.parse_module()
}

#[cfg(feature = "test")]
pub mod tests {
    use crate::println;
    use super::*;
    use alloc::string::ToString;
 
    fn ok(src: &str) -> Module {
        parse(src).unwrap_or_else(|e| panic!("expected parse success, got error: {}", e))
    }
    fn err(src: &str) -> String {
        parse(src).expect_err("expected parse error")
    }
 
    fn test_minimal_module() {
        let m = ok("module m; fn main() {}");
        assert_eq!(m.name, "m");
        assert_eq!(m.items.len(), 1);
    }
 
    fn test_missing_module_keyword_errors() {
        err("fn main() {}");
    }
 
    fn test_imports() {
        let m = ok("module m; import kernel; import other; fn main() {}");
        assert_eq!(m.imports, vec!["kernel".to_string(), "other".to_string()]);
    }
 
    fn test_function_with_params_and_return() {
        let m = ok("module m; fn add(a: u64, b: u64) -> u64 { return a + b; }");
        match &m.items[0] {
            Item::Function(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
                assert_eq!(f.params[0], ("a".to_string(), Type::U64));
                assert_eq!(f.ret, Type::U64);
            }
            _ => panic!("expected function"),
        }
    }
 
    fn test_function_void_return_defaults() {
        let m = ok("module m; fn f() { }");
        match &m.items[0] {
            Item::Function(f) => assert_eq!(f.ret, Type::Void),
            _ => panic!(),
        }
    }
 
    fn test_all_primitive_types_parse() {
        let src = "module m; fn f(a: u8, b: u16, c: u32, d: u64, e: i8, f2: i16, \
                    g: i32, h: i64, i: usize, j: isize, k: bool, l: char, s: string, v: void) {}";
        let m = ok(src);
        match &m.items[0] {
            Item::Function(f) => {
                let tys: Vec<&Type> = f.params.iter().map(|(_, t)| t).collect();
                assert_eq!(tys, vec![
                    &Type::U8, &Type::U16, &Type::U32, &Type::U64,
                    &Type::I8, &Type::I16, &Type::I32, &Type::I64,
                    &Type::Usize, &Type::Isize, &Type::Bool, &Type::Char,
                    &Type::Str, &Type::Void,
                ]);
            }
            _ => panic!(),
        }
    }
 
    fn test_pointer_type() {
        let m = ok("module m; fn f(p: u64 pointer) {}");
        match &m.items[0] {
            Item::Function(f) => assert_eq!(f.params[0].1, Type::Pointer(Box::new(Type::U64))),
            _ => panic!(),
        }
    }
 
    fn test_double_pointer_type() {
        let m = ok("module m; fn f(p: u64 pointer pointer) {}");
        match &m.items[0] {
            Item::Function(f) => assert_eq!(
                f.params[0].1,
                Type::Pointer(Box::new(Type::Pointer(Box::new(Type::U64))))
            ),
            _ => panic!(),
        }
    }
 
    fn test_array_type() {
        let m = ok("module m; fn f(a: u64[10]) {}");
        match &m.items[0] {
            Item::Function(f) => assert_eq!(f.params[0].1, Type::Array(Box::new(Type::U64), 10)),
            _ => panic!(),
        }
    }
 
    fn test_named_type() {
        let m = ok("module m; fn f(p: Point) {}");
        match &m.items[0] {
            Item::Function(f) => assert_eq!(f.params[0].1, Type::Named("Point".to_string())),
            _ => panic!(),
        }
    }
 
    fn test_struct_def() {
        let m = ok("module m; struct Point { x: u64; y: u64; } fn main() {}");
        match &m.items[0] {
            Item::Struct(s) => {
                assert_eq!(s.name, "Point");
                assert_eq!(s.fields, vec![
                    ("x".to_string(), Type::U64),
                    ("y".to_string(), Type::U64),
                ]);
            }
            _ => panic!("expected struct"),
        }
    }
 
    fn test_enum_def() {
        let m = ok("module m; enum State { Ready, Running, Dead } fn main() {}");
        match &m.items[0] {
            Item::Enum(e) => {
                assert_eq!(e.name, "State");
                assert_eq!(e.variants, vec!["Ready", "Running", "Dead"]);
            }
            _ => panic!("expected enum"),
        }
    }
 
    fn test_let_and_mut() {
        let m = ok("module m; fn f() { let x: u64 = 1; mut y: u64 = 2; }");
        match &m.items[0] {
            Item::Function(f) => {
                match &f.body[0] {
                    Stmt::Let { name, mutable, .. } => { assert_eq!(name, "x"); assert!(!mutable); }
                    _ => panic!(),
                }
                match &f.body[1] {
                    Stmt::Let { name, mutable, .. } => { assert_eq!(name, "y"); assert!(mutable); }
                    _ => panic!(),
                }
            }
            _ => panic!(),
        }
    }
 
    fn test_let_without_init() {
        let m = ok("module m; fn f() { let x: u64; }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::Let { init, .. } => assert!(init.is_none()),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
 
    fn test_assignment() {
        let m = ok("module m; fn f() { mut x: u64 = 1; x = 2; }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[1] {
                Stmt::Assign { .. } => {}
                _ => panic!("expected assign"),
            },
            _ => panic!(),
        }
    }
 
    fn test_if_else() {
        let m = ok("module m; fn f() { if 1 { } else { } }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::If { else_blk, .. } => assert!(else_blk.is_some()),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
 
    fn test_if_without_else() {
        let m = ok("module m; fn f() { if 1 { } }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::If { else_blk, .. } => assert!(else_blk.is_none()),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
 
    fn test_else_if_chain() {
        let m = ok("module m; fn f() { if 1 { } else if 2 { } else { } }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::If { else_blk, .. } => {
                    let eb = else_blk.as_ref().unwrap();
                    assert_eq!(eb.len(), 1);
                    assert!(matches!(eb[0], Stmt::If { .. }));
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    fn test_while_loop_break_continue() {
        let m = ok("module m; fn f() { while 1 { break; continue; } loop { break; } }");
        match &m.items[0] {
            Item::Function(f) => {
                assert!(matches!(f.body[0], Stmt::While { .. }));
                assert!(matches!(f.body[1], Stmt::Loop { .. }));
            }
            _ => panic!(),
        }
    }

    fn test_match_statement() {
        let src = "module m; enum S { A, B } fn f() { let s: S = S.A; \
                    match s { S.A => { } S.B => { } } }";
        let m = ok(src);
        match &m.items[1] {
            Item::Function(f) => match &f.body[1] {
                Stmt::Match { arms, .. } => assert_eq!(arms.len(), 2),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    fn test_operator_precedence() {
        let m = ok("module m; fn f() { let x: u64 = 1 + 2 * 3; }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::Let { init: Some(Expr::Binary(BinOp::Add, lhs, rhs)), .. } => {
                    assert!(matches!(**lhs, Expr::IntLit(1)));
                    assert!(matches!(**rhs, Expr::Binary(BinOp::Mul, _, _)));
                }
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }

    fn test_parens_override_precedence() {
        let m = ok("module m; fn f() { let x: u64 = (1 + 2) * 3; }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::Let { init: Some(Expr::Binary(BinOp::Mul, lhs, _)), .. } => {
                    assert!(matches!(**lhs, Expr::Binary(BinOp::Add, _, _)));
                }
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }

    fn test_all_binary_operators_parse() {
        let ops = ["+", "-", "*", "/", "%", "==", "!=", "<", ">", "<=", ">=",
                   "&&", "||", "&", "|", "^", "<<", ">>"];
        for op in ops {
            let src = format!("module m; fn f() {{ let x: u64 = 1 {} 2; }}", op);
            ok(&src);
        }
    }

    fn test_unary_operators() {
        let m = ok("module m; fn f() { let x: i64 = -1; let y: bool = !true; let z: u64 = ~1; }");
        match &m.items[0] {
            Item::Function(f) => {
                assert!(matches!(&f.body[0], Stmt::Let { init: Some(Expr::Unary(UnOp::Neg, _)), .. }));
                assert!(matches!(&f.body[1], Stmt::Let { init: Some(Expr::Unary(UnOp::Not, _)), .. }));
                assert!(matches!(&f.body[2], Stmt::Let { init: Some(Expr::Unary(UnOp::BitNot, _)), .. }));
            }
            _ => panic!(),
        }
    }

    fn test_address_of_and_deref() {
        let m = ok("module m; fn f() { let x: u64 = 1; let p: u64 pointer = &x; let y: u64 = *p; }");
        match &m.items[0] {
            Item::Function(f) => {
                assert!(matches!(&f.body[1], Stmt::Let { init: Some(Expr::AddrOf(_)), .. }));
                assert!(matches!(&f.body[2], Stmt::Let { init: Some(Expr::Deref(_)), .. }));
            }
            _ => panic!(),
        }
    }

    fn test_deref_assignment() {
        let m = ok("module m; fn f() { let p: u64 pointer = &p; *p = 5; }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[1] {
                Stmt::Assign { target: Expr::Deref(_), .. } => {}
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }

    fn test_field_and_arrow_field() {
        let m = ok("module m; fn f() { let x: u64 = p.a; let y: u64 = p->b; }");
        match &m.items[0] {
            Item::Function(f) => {
                assert!(matches!(&f.body[0], Stmt::Let { init: Some(Expr::Field(_, _)), .. }));
                assert!(matches!(&f.body[1], Stmt::Let { init: Some(Expr::ArrowField(_, _)), .. }));
            }
            _ => panic!(),
        }
    }

    fn test_indexing() {
        let m = ok("module m; fn f() { let x: u64 = arr[3]; }");
        match &m.items[0] {
            Item::Function(f) => assert!(matches!(&f.body[0], Stmt::Let { init: Some(Expr::Index(_, _)), .. })),
            _ => panic!(),
        }
    }

    fn test_array_literal() {
        let m = ok("module m; fn f() { let a: u64[3] = [1,2,3]; }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::Let { init: Some(Expr::ArrayLit(items)), .. } => assert_eq!(items.len(), 3),
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }

    fn test_function_call() {
        let m = ok("module m; fn f() { let x: u64 = add(1, 2); }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::Let { init: Some(Expr::Call(name, args)), .. } => {
                    assert_eq!(name, "add");
                    assert_eq!(args.len(), 2);
                }
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }

    fn test_kernel_call() {
        let m = ok(r#"module m; fn f() { kernel.print("hi"); }"#);
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::ExprStmt(Expr::KernelCall(name, args)) => {
                    assert_eq!(name, "print");
                    assert_eq!(args.len(), 1);
                }
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }

    fn test_enum_variant_expr() {
        let m = ok("module m; fn f() { let s: State = State.Ready; }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::Let { init: Some(Expr::EnumVariant(e, v)), .. } => {
                    assert_eq!(e, "State");
                    assert_eq!(v, "Ready");
                }
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }
    
    fn test_lowercase_dot_is_field_not_enum_variant() {
        let m = ok("module m; fn f() { let x: u64 = point.x; }");
        match &m.items[0] {
            Item::Function(f) => assert!(matches!(&f.body[0], Stmt::Let { init: Some(Expr::Field(_, _)), .. })),
            _ => panic!(),
        }
    }
 
    fn test_sizeof() {
        let m = ok("module m; fn f() { let x: u64 = sizeof(u64); }");
        match &m.items[0] {
            Item::Function(f) => match &f.body[0] {
                Stmt::Let { init: Some(Expr::Sizeof(Type::U64)), .. } => {}
                other => panic!("unexpected: {:?}", other),
            },
            _ => panic!(),
        }
    }

    fn test_postfix_incr_decr() {
        let m = ok("module m; fn f() { mut x: u64 = 0; x++; x--; }");
        match &m.items[0] {
            Item::Function(f) => {
                assert!(matches!(&f.body[1], Stmt::ExprStmt(Expr::PostIncr(_))));
                assert!(matches!(&f.body[2], Stmt::ExprStmt(Expr::PostDecr(_))));
            }
            _ => panic!(),
        }
    }

    fn test_bool_literals() {
        let m = ok("module m; fn f() { let a: bool = true; let b: bool = false; }");
        match &m.items[0] {
            Item::Function(f) => {
                assert!(matches!(&f.body[0], Stmt::Let { init: Some(Expr::BoolLit(true)), .. }));
                assert!(matches!(&f.body[1], Stmt::Let { init: Some(Expr::BoolLit(false)), .. }));
            }
            _ => panic!(),
        }
    }

    fn test_char_and_hex_literals() {
        let m = ok("module m; fn f() { let a: char = 'x'; let b: u64 = 0xFF; }");
        match &m.items[0] {
            Item::Function(f) => {
                assert!(matches!(&f.body[0], Stmt::Let { init: Some(Expr::CharLit(b'x')), .. }));
                assert!(matches!(&f.body[1], Stmt::Let { init: Some(Expr::IntLit(255)), .. }));
            }
            _ => panic!(),
        }
    }

    fn test_return_with_and_without_value() {
        let m = ok("module m; fn f() -> u64 { return 1; } fn g() { return; }");
        match &m.items[0] {
            Item::Function(f) => assert!(matches!(&f.body[0], Stmt::Return(Some(_)))),
            _ => panic!(),
        }
        match &m.items[1] {
            Item::Function(f) => assert!(matches!(&f.body[0], Stmt::Return(None))),
            _ => panic!(),
        }
    }

    fn test_line_comments_and_block_comments_ignored() {
        let m = ok("module m; // comment\n /* block */ fn main() {}");
        assert_eq!(m.items.len(), 1);
    }

    fn test_missing_semicolon_errors() {
        err("module m; fn f() { let x: u64 = 1 }");
    }

    fn test_unclosed_brace_errors() {
        err("module m; fn f() {");
    }

    fn test_missing_type_errors() {
        err("module m; fn f(a: ) {}");
    }

    fn test_full_program_all_constructs() {
        let src = r#"
            module full;
            import kernel;
 
            struct Point { x: u64; y: u64; }
            enum State { Ready, Running, Dead }
 
            fn add(a: u64, b: u64) -> u64 {
                return a + b;
            }
 
            fn main() {
                let x: u64 = 10;
                mut y: u64 = 20;
                y = y + x;
                let arr: u64[3] = [1, 2, 3];
                let p: Point;
                let addr: Point pointer = &p;
                let s: State = State.Ready;
                match s {
                    State.Ready => { kernel.print("ready"); }
                    State.Running => { }
                    State.Dead => { }
                }
                mut i: u64 = 0;
                while i < 3 {
                    i++;
                }
                loop {
                    break;
                }
                if x > 5 {
                } else if x == 5 {
                } else {
                }
                kernel.exit(0);
            }
        "#;
        let m = ok(src);
        assert_eq!(m.items.len(), 4);
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_minimal_module,
            &test_missing_module_keyword_errors,
            &test_imports,
            &test_function_with_params_and_return,
            &test_function_void_return_defaults,
            &test_all_primitive_types_parse,
            &test_pointer_type,
            &test_double_pointer_type,
            &test_array_type,
            &test_named_type,
            &test_struct_def,
            &test_enum_def,
            &test_let_and_mut,
            &test_let_without_init,
            &test_assignment,
            &test_if_else,
            &test_if_without_else,
            &test_else_if_chain,
            &test_while_loop_break_continue,
            &test_match_statement,
            &test_operator_precedence,
            &test_parens_override_precedence,
            &test_all_binary_operators_parse,
            &test_unary_operators,
            &test_address_of_and_deref,
            &test_deref_assignment,
            &test_field_and_arrow_field,
            &test_indexing,
            &test_array_literal,
            &test_function_call,
            &test_kernel_call,
            &test_enum_variant_expr,
            &test_lowercase_dot_is_field_not_enum_variant,
            &test_sizeof,
            &test_postfix_incr_decr,
            &test_bool_literals,
            &test_char_and_hex_literals,
            &test_return_with_and_without_value,
            &test_line_comments_and_block_comments_ignored,
            &test_missing_semicolon_errors,
            &test_unclosed_brace_errors,
            &test_missing_type_errors,
            &test_full_program_all_constructs,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("OK");
    }
}
