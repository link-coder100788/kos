use alloc::string::{String, ToString};

use crate::{filesys::{backend::FsError, fs_ipc::{FsRequest, FsResponse, fs_call}, }, shell::fs::current_cwd, k::{aarch64_codegen, ir, kbin, parser, typecheck}, println};

pub enum KShellCommand {
    Build(String),
    Run(String),
    Ast(String),
    Tokenize(String),
    Modules(String),
    Semantic(String),
    IR(String),
    Disassemble(String),
}

impl KShellCommand {
    pub fn parse(s: &str) -> Option<Self> {
        let (cmd, args) = s.trim().split_once(' ').unwrap_or((s.trim(), ""));
        let args = args.trim().to_string();
        match cmd {
            "build" => Some(Self::Build(args)),
            "run" => Some(Self::Run(args)),
            "ast" => Some(Self::Ast(args)),
            "tokenize" => Some(Self::Tokenize(args)),
            "modules" => Some(Self::Modules(args)),
            "semantic" => Some(Self::Semantic(args)),
            "ir" => Some(Self::IR(args)),
            "disassemble" => Some(Self::Disassemble(args)),
            _ => None,
        }
    }

    pub fn execute(&self) {
        match self {
            KShellCommand::Build(p) => {
                println!("Building {}", p);
                let src = match fs_call(FsRequest::Read { cwd: current_cwd(), path: p.clone() }) {
                    FsResponse::Data(data) => core::str::from_utf8(&data).expect("Invalid utf8").to_string(),
                    FsResponse::Err(e) => { println!("Error while reading {}: {:?}", p, e); return; }
                    _ => { return; }
                };
                println!("Extracted source");
                let module = match parser::parse(&src) {
                    Ok(module) => module,
                    Err(e) => { println!("Parse error: {}", e); return; }
                };
                println!("Parsed module: {}", module.name);
                let sema = match typecheck::analyze(&module) {
                    Ok(sema) => sema,
                    Err(e) => { println!("Type error: {}", e); return; }
                };
                println!("Typecheck complete");
                let ir_prog = ir::lower(&module, &sema);
                println!("IR program generated");
                let out = aarch64_codegen::generate(&ir_prog);
                println!("Codegen complete");
                let bin = kbin::write(out.entry_address, &out.code, &out.data);
                match fs_call(FsRequest::Write { cwd: current_cwd(), path: String::from("out.kbin"), data: bin }) {
                    FsResponse::Ok => { println!("Successfully wrote out.kbin"); }
                    FsResponse::Err(e) => { println!("Error while writing out.kbin: {:?}", e); }
                    _ => { return; }
                }
                println!("Wrote out.kbin");
            }
            KShellCommand::Run(p) => {
                println!("Running: {}", p);
                crate::loader::run_foreground(p.clone(), String::new());
            }
            KShellCommand::Ast(p) => {
                
            }
            KShellCommand::Tokenize(p) => {
                
            }
            KShellCommand::Modules(p) => {
                
            }
            KShellCommand::Semantic(p) => {
                
            }
            KShellCommand::IR(p) => {
                
            }
            KShellCommand::Disassemble(p) => {
                
            }
        }
    }
}
