//! ast.rs — Abstract Syntax Tree for the K language (KLS v0.2)

use alloc::string::String;
use alloc::boxed::Box;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    U8, U16, U32, U64,
    I8, I16, I32, I64,
    Usize, Isize,
    Bool, Char, Str, Void,
    Pointer(Box<Type>),
    Array(Box<Type>, u64),
    Named(String),
}

impl Type {
    /// Size in bytes on AArch64 (usize/isize = 64-bit).
    pub fn size(&self) -> u64 {
        match self {
            Type::U8 | Type::I8 | Type::Bool | Type::Char => 1,
            Type::U16 | Type::I16 => 2,
            Type::U32 | Type::I32 => 4,
            Type::U64 | Type::I64 | Type::Usize | Type::Isize => 8,
            Type::Str => 8,
            Type::Void => 0,
            Type::Pointer(_) => 8,
            Type::Array(t, n) => t.size() * n,
            Type::Named(_) => 8,
        }
    }

    pub fn is_signed(&self) -> bool {
        matches!(self, Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Isize)
    }
}

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub imports: Vec<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Function(Function),
    Struct(StructDef),
    Enum(EnumDef),
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Type)>,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub ret: Type,
    pub body: Block,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone)]
pub enum Stmt {
    Let { name: String, mutable: bool, ty: Type, init: Option<Expr> },
    Assign { target: Expr, value: Expr },
    ExprStmt(Expr),
    Return(Option<Expr>),
    If { cond: Expr, then_blk: Block, else_blk: Option<Block> },
    While { cond: Expr, body: Block },
    Loop { body: Block },
    Break,
    Continue,
    Match { scrutinee: Expr, arms: Vec<(String, Block)> },
}

#[derive(Debug, Clone)]
pub enum Expr {
    IntLit(u64),
    BoolLit(bool),
    StrLit(String),
    CharLit(u8),
    Ident(String),
    ArrayLit(Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Field(Box<Expr>, String),
    ArrowField(Box<Expr>, String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    KernelCall(String, Vec<Expr>),
    AddrOf(Box<Expr>),
    Deref(Box<Expr>),
    EnumVariant(String, String),
    Sizeof(Type),
    PostIncr(Box<Expr>),
    PostDecr(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp { Neg, Not, BitNot }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Gt, Le, Ge,
    And, Or,
    BitAnd, BitOr, BitXor, Shl, Shr,
}

#[cfg(feature = "test")]
pub mod tests {
    use crate::println;
    use super::*;
    use alloc::string::ToString;
 
    fn test_integer_type_sizes() {
        assert_eq!(Type::U8.size(), 1);
        assert_eq!(Type::U16.size(), 2);
        assert_eq!(Type::U32.size(), 4);
        assert_eq!(Type::U64.size(), 8);
        assert_eq!(Type::I8.size(), 1);
        assert_eq!(Type::I16.size(), 2);
        assert_eq!(Type::I32.size(), 4);
        assert_eq!(Type::I64.size(), 8);
        assert_eq!(Type::Usize.size(), 8);
        assert_eq!(Type::Isize.size(), 8);
    }
 
    fn test_other_type_sizes() {
        assert_eq!(Type::Bool.size(), 1);
        assert_eq!(Type::Char.size(), 1);
        assert_eq!(Type::Str.size(), 8);
        assert_eq!(Type::Void.size(), 0);
        assert_eq!(Type::Pointer(Box::new(Type::U8)).size(), 8);
    }
 
    fn test_array_type_size_is_elem_size_times_count() {
        assert_eq!(Type::Array(Box::new(Type::U32), 5).size(), 20);
        assert_eq!(Type::Array(Box::new(Type::U8), 0).size(), 0);
    }
 
    fn test_named_type_placeholder_size() {
        assert_eq!(Type::Named("Foo".to_string()).size(), 8);
    }
 
    fn test_is_signed() {
        assert!(Type::I8.is_signed());
        assert!(Type::I16.is_signed());
        assert!(Type::I32.is_signed());
        assert!(Type::I64.is_signed());
        assert!(Type::Isize.is_signed());
        assert!(!Type::U8.is_signed());
        assert!(!Type::U64.is_signed());
        assert!(!Type::Usize.is_signed());
        assert!(!Type::Bool.is_signed());
        assert!(!Type::Pointer(Box::new(Type::I64)).is_signed());
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_integer_type_sizes,
            &test_other_type_sizes,
            &test_array_type_size_is_elem_size_times_count,
            &test_named_type_placeholder_size,
            &test_is_signed,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("OK");
    }
}
