//! lexer.rs — Tokenizer for the K language

use alloc::string::String;
use alloc::vec::Vec;
use alloc::string::ToString;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // literals
    Int(u64),
    Str(String),
    Char(u8),
    Ident(String),

    // keywords
    Module, Import, Fn, Return, Let, Mut, If, Else, While, Loop, For, In,
    Struct, Enum, Match, Break, Continue, Pointer, Sizeof, True, False,

    // punctuation
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Semi, Colon, Comma, Dot, Arrow, FatArrow,

    // operators
    Plus, Minus, Star, Slash, Percent,
    EqEq, NotEq, Lt, Gt, Le, Ge,
    AndAnd, OrOr, Not,
    Amp, Pipe, Caret, Tilde, Shl, Shr,
    Eq,
    PlusPlus, MinusMinus,

    Eof,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    pub line: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer { src: src.as_bytes(), pos: 0, line: 1 }
    }

    fn peek(&self) -> u8 {
        if self.pos < self.src.len() { self.src[self.pos] } else { 0 }
    }
    fn peek_at(&self, off: usize) -> u8 {
        let p = self.pos + off;
        if p < self.src.len() { self.src[p] } else { 0 }
    }
    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos += 1;
        if c == b'\n' { self.line += 1; }
        c
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while self.peek().is_ascii_whitespace() { self.bump(); }
            if self.peek() == b'/' && self.peek_at(1) == b'/' {
                while self.peek() != b'\n' && self.peek() != 0 { self.bump(); }
                continue;
            }
            if self.peek() == b'/' && self.peek_at(1) == b'*' {
                self.bump(); self.bump();
                while !(self.peek() == b'*' && self.peek_at(1) == b'/') && self.peek() != 0 {
                    self.bump();
                }
                if self.peek() != 0 { self.bump(); self.bump(); }
                continue;
            }
            break;
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<(Tok, usize)>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_ws_and_comments();
            let line = self.line;
            let c = self.peek();
            if c == 0 { out.push((Tok::Eof, line)); break; }

            if c.is_ascii_digit() {
                out.push((self.lex_number(), line));
                continue;
            }
            if c == b'"' {
                out.push((self.lex_string(), line));
                continue;
            }
            if c == b'\'' {
                out.push((self.lex_char(), line));
                continue;
            }
            if c.is_ascii_alphabetic() || c == b'_' {
                out.push((self.lex_ident_or_kw(), line));
                continue;
            }

            // operators / punctuation
            let tok = match c {
                b'(' => { self.bump(); Tok::LParen }
                b')' => { self.bump(); Tok::RParen }
                b'{' => { self.bump(); Tok::LBrace }
                b'}' => { self.bump(); Tok::RBrace }
                b'[' => { self.bump(); Tok::LBracket }
                b']' => { self.bump(); Tok::RBracket }
                b';' => { self.bump(); Tok::Semi }
                b':' => { self.bump(); Tok::Colon }
                b',' => { self.bump(); Tok::Comma }
                b'.' => { self.bump(); Tok::Dot }
                b'+' => {
                    self.bump();
                    if self.peek() == b'+' { self.bump(); Tok::PlusPlus } else { Tok::Plus }
                }
                b'-' => {
                    self.bump();
                    if self.peek() == b'-' { self.bump(); Tok::MinusMinus }
                    else if self.peek() == b'>' { self.bump(); Tok::Arrow }
                    else { Tok::Minus }
                }
                b'*' => { self.bump(); Tok::Star }
                b'/' => { self.bump(); Tok::Slash }
                b'%' => { self.bump(); Tok::Percent }
                b'=' => {
                    self.bump();
                    if self.peek() == b'=' { self.bump(); Tok::EqEq }
                    else if self.peek() == b'>' { self.bump(); Tok::FatArrow }
                    else { Tok::Eq }
                }
                b'!' => {
                    self.bump();
                    if self.peek() == b'=' { self.bump(); Tok::NotEq } else { Tok::Not }
                }
                b'<' => {
                    self.bump();
                    if self.peek() == b'=' { self.bump(); Tok::Le }
                    else if self.peek() == b'<' { self.bump(); Tok::Shl }
                    else { Tok::Lt }
                }
                b'>' => {
                    self.bump();
                    if self.peek() == b'=' { self.bump(); Tok::Ge }
                    else if self.peek() == b'>' { self.bump(); Tok::Shr }
                    else { Tok::Gt }
                }
                b'&' => {
                    self.bump();
                    if self.peek() == b'&' { self.bump(); Tok::AndAnd } else { Tok::Amp }
                }
                b'|' => {
                    self.bump();
                    if self.peek() == b'|' { self.bump(); Tok::OrOr } else { Tok::Pipe }
                }
                b'^' => { self.bump(); Tok::Caret }
                b'~' => { self.bump(); Tok::Tilde }
                other => {
                    return Err(alloc::format!("lexer: unexpected byte {:?} ({}) at line {}", other as char, other, line));
                }
            };
            out.push((tok, line));
        }
        Ok(out)
    }

    fn lex_number(&mut self) -> Tok {
        let start = self.pos;
        if self.peek() == b'0' && (self.peek_at(1) == b'x' || self.peek_at(1) == b'X') {
            self.bump(); self.bump();
            while self.peek().is_ascii_hexdigit() { self.bump(); }
            let s = core::str::from_utf8(&self.src[start + 2..self.pos]).unwrap();
            return Tok::Int(u64::from_str_radix(s, 16).unwrap());
        }
        while self.peek().is_ascii_digit() { self.bump(); }
        let s = core::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Tok::Int(s.parse().unwrap())
    }

    fn lex_string(&mut self) -> Tok {
        self.bump(); // opening quote
        let mut s = String::new();
        while self.peek() != b'"' && self.peek() != 0 {
            let c = self.bump();
            if c == b'\\' {
                let e = self.bump();
                s.push(match e {
                    b'n' => '\n', b't' => '\t', b'0' => '\0', b'\\' => '\\', b'"' => '"',
                    other => other as char,
                });
            } else {
                s.push(c as char);
            }
        }
        self.bump(); // closing quote
        Tok::Str(s)
    }

    fn lex_char(&mut self) -> Tok {
        self.bump(); // opening '
        let c = self.bump();
        let val = if c == b'\\' {
            let e = self.bump();
            match e { b'n' => b'\n', b't' => b'\t', b'0' => 0, other => other }
        } else { c };
        self.bump(); // closing '
        Tok::Char(val)
    }

    fn lex_ident_or_kw(&mut self) -> Tok {
        let start = self.pos;
        while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' { self.bump(); }
        let s = core::str::from_utf8(&self.src[start..self.pos]).unwrap();
        match s {
            "module" => Tok::Module,
            "import" => Tok::Import,
            "fn" => Tok::Fn,
            "return" => Tok::Return,
            "let" => Tok::Let,
            "mut" => Tok::Mut,
            "if" => Tok::If,
            "else" => Tok::Else,
            "while" => Tok::While,
            "loop" => Tok::Loop,
            "for" => Tok::For,
            "in" => Tok::In,
            "struct" => Tok::Struct,
            "enum" => Tok::Enum,
            "match" => Tok::Match,
            "break" => Tok::Break,
            "continue" => Tok::Continue,
            "pointer" => Tok::Pointer,
            "sizeof" => Tok::Sizeof,
            "true" => Tok::True,
            "false" => Tok::False,
            _ => Tok::Ident(s.to_string()),
        }
    }
}

#[cfg(feature = "test")]
pub mod tests {
    use crate::{println, test};
    use super::*;
    use alloc::vec;

    fn toks(src: &str) -> Vec<Tok> {
        Lexer::new(src).tokenize().unwrap().into_iter().map(|(t, _)| t).collect()
    }

    fn test_empty_input() {
        assert_eq!(toks(""), vec![Tok::Eof]);
    }

    fn test_whitespace_only() {
        assert_eq!(toks("   \n\t  \n"), vec![Tok::Eof]);
    }

    fn test_line_comment() {
        assert_eq!(toks("// hello\n42"), vec![Tok::Int(42), Tok::Eof]);
    }

    fn test_block_comment() {
        assert_eq!(toks("/* a\nb */42"), vec![Tok::Int(42), Tok::Eof]);
    }

    fn test_unterminated_block_comment_does_not_hang() {
        assert_eq!(toks("/* never closed"), vec![Tok::Eof]);
    }

    fn test_decimal_int() {
        assert_eq!(toks("12345"), vec![Tok::Int(12345), Tok::Eof]);
    }

    fn test_hex_int() {
        assert_eq!(toks("0xFF"), vec![Tok::Int(255), Tok::Eof]);
        assert_eq!(toks("0x0"), vec![Tok::Int(0), Tok::Eof]);
    }

    fn test_string_literal_basic() {
        assert_eq!(toks("\"hi\""), vec![Tok::Str("hi".to_string()), Tok::Eof]);
    }

    fn test_string_literal_escapes() {
        assert_eq!(toks(r#""a\nb\tc\\d\"e""#), vec![Tok::Str("a\nb\tc\\d\"e".to_string()), Tok::Eof]);
    }

    fn test_char_literal() {
        assert_eq!(toks("'a'"), vec![Tok::Char(b'a'), Tok::Eof]);
        assert_eq!(toks("'\\n'"), vec![Tok::Char(b'\n'), Tok::Eof]);
        assert_eq!(toks("'\\0'"), vec![Tok::Char(0), Tok::Eof]);
    }

    fn test_all_keywords() {
        let src = "module import fn return let mut if else while loop for in \
                    struct enum match break continue pointer sizeof true false";
        assert_eq!(toks(src), vec![
            Tok::Module, Tok::Import, Tok::Fn, Tok::Return, Tok::Let, Tok::Mut,
            Tok::If, Tok::Else, Tok::While, Tok::Loop, Tok::For, Tok::In,
            Tok::Struct, Tok::Enum, Tok::Match, Tok::Break, Tok::Continue,
            Tok::Pointer, Tok::Sizeof, Tok::True, Tok::False, Tok::Eof,
        ]);
    }

    fn test_identifiers() {
        assert_eq!(toks("foo _bar baz123 _"), vec![
            Tok::Ident("foo".into()), Tok::Ident("_bar".into()),
            Tok::Ident("baz123".into()), Tok::Ident("_".into()), Tok::Eof,
        ]);
    }

    fn test_punctuation() {
        assert_eq!(toks("(){}[];:,.  =>  ->"), vec![
            Tok::LParen, Tok::RParen, Tok::LBrace, Tok::RBrace,
            Tok::LBracket, Tok::RBracket, Tok::Semi, Tok::Colon, Tok::Comma, Tok::Dot,
            Tok::FatArrow, Tok::Arrow, Tok::Eof,
        ]);
    }

    fn test_arithmetic_operators() {
        assert_eq!(toks("+ - * / %"), vec![
            Tok::Plus, Tok::Minus, Tok::Star, Tok::Slash, Tok::Percent, Tok::Eof,
        ]);
    }

    fn test_increment_decrement_vs_minus() {
        assert_eq!(toks("++ -- + -"), vec![
            Tok::PlusPlus, Tok::MinusMinus, Tok::Plus, Tok::Minus, Tok::Eof,
        ]);
    }

    fn test_comparison_operators() {
        assert_eq!(toks("== != < > <= >="), vec![
            Tok::EqEq, Tok::NotEq, Tok::Lt, Tok::Gt, Tok::Le, Tok::Ge, Tok::Eof,
        ]);
    }

    fn test_logical_operators() {
        assert_eq!(toks("&& || !"), vec![Tok::AndAnd, Tok::OrOr, Tok::Not, Tok::Eof]);
    }

    fn test_bitwise_operators() {
        assert_eq!(toks("& | ^ ~ << >>"), vec![
            Tok::Amp, Tok::Pipe, Tok::Caret, Tok::Tilde, Tok::Shl, Tok::Shr, Tok::Eof,
        ]);
    }

    fn test_assignment_vs_equality() {
        assert_eq!(toks("= =="), vec![Tok::Eq, Tok::EqEq, Tok::Eof]);
    }

    fn test_arrow_vs_minus() {
        assert_eq!(toks("p->x - 1"), vec![
            Tok::Ident("p".into()), Tok::Arrow, Tok::Ident("x".into()),
            Tok::Minus, Tok::Int(1), Tok::Eof,
        ]);
    }

    fn test_line_tracking() {
        let toks_with_lines = Lexer::new("a\nb\nc").tokenize().unwrap();
        let lines: Vec<usize> = toks_with_lines.iter().map(|(_, l)| *l).collect();
        assert_eq!(lines, vec![1, 2, 3, 3]); // a=1, b=2, c=3, Eof=3
    }

    fn test_full_function_snippet() {
        let src = "fn add(a: u64, b: u64) -> u64 { return a + b; }";
        let t = toks(src);
        assert_eq!(t[0], Tok::Fn);
        assert_eq!(t[1], Tok::Ident("add".into()));
        assert_eq!(t[2], Tok::LParen);
        assert_eq!(*t.last().unwrap(), Tok::Eof);
        assert!(t.contains(&Tok::Arrow));
        assert!(t.contains(&Tok::Return));
    }

    fn test_unexpected_byte_errors() {
        let e = Lexer::new("@").tokenize().unwrap_err();
        assert!(e.contains("unexpected byte"));
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_empty_input,
            &test_whitespace_only,
            &test_line_comment,
            &test_block_comment,
            &test_unterminated_block_comment_does_not_hang,
            &test_decimal_int,
            &test_hex_int,
            &test_string_literal_basic,
            &test_string_literal_escapes,
            &test_char_literal,
            &test_all_keywords,
            &test_identifiers,
            &test_punctuation,
            &test_arithmetic_operators,
            &test_increment_decrement_vs_minus,
            &test_comparison_operators,
            &test_logical_operators,
            &test_bitwise_operators,
            &test_assignment_vs_equality,
            &test_arrow_vs_minus,
            &test_line_tracking,
            &test_full_function_snippet,
            &test_unexpected_byte_errors,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("OK");
    }
}
