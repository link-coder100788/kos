//! The shell's own file and program commands (`cd`, `write`, `run`, `bg`,
//! `open`), running programs from `/bin` by name, and pipelines of them
//! (`grep a notes.txt | wc > count.txt`). `ls`, `cat`, `rm` and the like
//! are programs in `/bin`. These are clients of `fs_task` via `fs_call`.

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::println;
use crate::std::lock::mutex::Mutex;

pub enum ShellCommand {
    Cd(String),
    Write(String, String),
    /// A program and its arguments.
    Run(String, String),
    Bg(String, String),
    Open(String),
    Help,
}

// The shell's current-directory state lives here, with the caller
// (uart_read_task), not in fs_task -- fs_task stays a stateless server.
// If you add more shells/ttys later, give each its own cwd variable.
/// Empty means "/".
pub static SHELL_CWD: Mutex<String> = Mutex::new(String::new());

pub fn current_cwd() -> String {
    let cwd = SHELL_CWD.lock();
    if cwd.is_empty() { "/".to_string() } else { cwd.clone() }
}

fn set_cwd(path: String) {
    *SHELL_CWD.lock() = path;
}

/// Where system commands (userspace/cmd/) live on the disk.
pub const BIN_DIR: &str = "/bin";
/// Where the userspace programs (userspace/prog/) live: the Programs window
/// lists them.
pub const PROGRAMS_DIR: &str = "/programs";

/// `"prog arg1 arg2"` -> `("prog", "arg1 arg2")`.
fn split_command(s: &str) -> (&str, &str) {
    match s.split_once(' ') {
        Some((cmd, rest)) => (cmd, rest.trim()),
        None => (s, ""),
    }
}

/// The path of command `name` in `/bin` or else `/programs`
/// (`<dir>/<name>.elf.b64`), if it's there.
fn find_command(name: &str) -> Option<String> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let file = alloc::format!("{}.elf.b64", name);
    [BIN_DIR, PROGRAMS_DIR].into_iter().find_map(|dir| match fs_call(FsRequest::List { cwd: "/".to_string(), path: dir.to_string() }) {
        FsResponse::Listing(entries) if entries.iter().any(|e| !e.is_dir && e.name == file) => Some(alloc::format!("{}/{}", dir, file)),
        _ => None,
    })
}

/// What `run` / `bg` were given: a bare name (no extension, so it can't be
/// a program file) is looked up like a command; anything else is a path.
fn program_path(given: &str) -> String {
    if given.contains('.') {
        given.to_string()
    } else {
        find_command(given).unwrap_or_else(|| given.to_string())
    }
}

impl ShellCommand {
    /// Never panics on malformed input -- worst case returns None (after
    /// printing why) so a shell typo can't take down the whole kernel.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (cmd, rest) = match s.split_once(' ') {
            Some((c, r)) => (c, r.trim()),
            None => (s, ""),
        };
        match cmd {
            "cd" => Some(ShellCommand::Cd(if rest.is_empty() { "/".to_string() } else { rest.to_string() })),
            "write" => match rest.split_once(' ') {
                Some((path, content)) => Some(ShellCommand::Write(path.to_string(), content.to_string())),
                None => {
                    println!("write: usage: write <path> <content>");
                    None
                }
            },
            "run" => {
                if rest.is_empty() {
                    println!("run: missing path");
                    None
                } else {
                    let (path, args) = split_command(rest);
                    Some(ShellCommand::Run(program_path(path), args.to_string()))
                }
            },
            "bg" => {
                if rest.is_empty() {
                    println!("bg: missing path");
                    None
                } else {
                    let (path, args) = split_command(rest);
                    Some(ShellCommand::Bg(program_path(path), args.to_string()))
                }
            },
            "open" => {
                if rest.is_empty() {
                    println!("open: missing path");
                    None
                } else {
                    Some(ShellCommand::Open(rest.to_string()))
                }
            },
            // Anything else may be a command in /bin.
            _ => match find_command(cmd) {
                Some(path) => Some(ShellCommand::Run(path, rest.to_string())),
                None => Some(ShellCommand::Help),
            },
        }
    }

    pub fn execute(&self) {
        let cwd = current_cwd();
        match self {
            ShellCommand::Help => {
                println!("Available commands:");
                println!("  cd <path>");
                println!("  write <path> <data>");
                println!("  run <path|command> [args]  run a program; the shell waits until it ends");
                println!("  bg <path|command> [args]   run a program in the background, without the console");
                println!("  <command> [args]   run <command>.elf.b64 from /bin or /programs: ls, cat, mkdir, rm,");
                println!("                     ps, kill, grep, wc, encode, decode, ... (Ctrl+D ends typed input)");
                println!("  a | b, < in, > out, >> out, 2> err, 2>&1   join programs, or send them to files");
                println!("  open <image.bmp/png/jpeg/heic/mp4/mov, or sound.wav>");
                println!("  k <build|run> <file>");
                println!("Network: net, ping <host>, dns <host or url>, fetch <url>, browse [url]");
                println!("Sound: beep, snd <hz> (snd 0 stops), play <notes or file.wav> (e.g. play T140 O4 C D E F G2)");
                println!("Assistant: kai <request> (try `kai` alone), kai --debug <request>");
                println!("Debugging: log [target level], dmesg [count] [text], stats");
                println!("System: mmu [addr], heap_remain, heap_test, load [seconds]");
                println!("  help");
            }
            ShellCommand::Cd(path) => match fs_call(FsRequest::ChangeDir { cwd: cwd.clone(), path: path.clone() }) {
                FsResponse::Path(new_cwd) => set_cwd(new_cwd),
                FsResponse::Err(e) => { println!("cd: err: {:?}", e); },
                _ => { println!("cd: unexpected response"); },
            },
            ShellCommand::Write(path, content) => {
                let data = Vec::from(content.as_bytes());
                match fs_call(FsRequest::Write { cwd, path: path.clone(), data }) {
                    FsResponse::Ok => { println!("write: ok"); },
                    FsResponse::Err(e) => { println!("write: err: {:?}", e); },
                    _ => { println!("write: unexpected response"); },
                }
            },
            ShellCommand::Run(path, args) => crate::loader::run_foreground(path.clone(), args.clone()),
            ShellCommand::Bg(path, args) => {
                crate::loader::run_background(path.clone(), args.clone());
                println!("bg: started {}", path);
            },
            ShellCommand::Open(path) => {
                let (_, ext) = path.split_once(".").unwrap_or(("", ""));
                match ext {
                    "bmp" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_bmp(&bytes) {
                            Ok(image) => {
                                let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                                crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    "png" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_png(&bytes) {
                            Ok(image) => {
                                let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                                crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    "jpeg" | "jpg" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_jpeg(&bytes) {
                           Ok(image) => {
                               let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                               crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                           }
                           Err(e) => println!("open: {}", e),
                       }
                    }
                    "heic" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::img::decode_heic(&bytes) {
                            Ok(image) => {
                                let viewer = crate::ui::apps::image::ImageViewer::new(path.clone(), image);
                                crate::ui::window::open_app(alloc::boxed::Box::new(viewer));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    "mp4" | "mov" => {
                        let bytes = match fs_call(FsRequest::Read { cwd: cwd.clone(), path: path.clone() }) {
                            FsResponse::Data(bytes) => bytes,
                            FsResponse::Err(e) => { println!("open: err: {:?}", e); return; }
                            _ => { println!("open: unexpected response"); return; }
                        };
                        match crate::ui::video::Mp4Decoder::new(bytes) {
                            Ok(decoder) => {
                                let player = crate::ui::apps::video::VideoPlayer::new(path.clone(), alloc::boxed::Box::new(decoder));
                                crate::ui::window::open_app(alloc::boxed::Box::new(player));
                            }
                            Err(e) => println!("open: {}", e),
                        }
                    }
                    "wav" | "WAV" => {
                        if let Err(e) = crate::sound::play_file(&cwd, path) {
                            println!("open: {}", e);
                        }
                    }
                    _ => { println!("open: unknown extension: {}", ext); }
                }
            }
        }
    }
}

// ---- pipelines -------------------------------------------------------------

/// One program of a pipeline as typed: its words, and its redirections.
#[derive(Debug, PartialEq, Default)]
pub struct StageSpec {
    pub words: Vec<String>,
    /// `< file`
    pub input: Option<String>,
    /// `> file`, or `>> file` (true: add to it).
    pub output: Option<(String, bool)>,
    /// `2> file` or `2>> file`.
    pub errors: Option<(String, bool)>,
    /// `2>&1`
    pub errors_to_output: bool,
}

/// Splits a line at `|` into programs and takes out their redirections.
/// None if it has no `|`, `<` or `>` (so it's an ordinary command).
pub fn parse_pipeline(line: &str) -> Option<Result<Vec<StageSpec>, String>> {
    if !line.contains(['|', '<', '>']) {
        return None;
    }
    // Words, with the operators as words of their own.
    let mut tokens: Vec<String> = Vec::new();
    let mut word = String::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let end_word = |word: &mut String, tokens: &mut Vec<String>| {
        if !word.is_empty() {
            tokens.push(core::mem::take(word));
        }
    };
    while i < bytes.len() {
        let rest = &line[i..];
        let op = if word.is_empty() && rest.starts_with("2>&1") { Some("2>&1") }
            else if word.is_empty() && rest.starts_with("2>>") { Some("2>>") }
            else if word.is_empty() && rest.starts_with("2>") { Some("2>") }
            else if rest.starts_with(">>") { Some(">>") }
            else if rest.starts_with(['>', '<', '|']) { Some(&rest[..1]) }
            else { None };
        if let Some(op) = op {
            end_word(&mut word, &mut tokens);
            tokens.push(op.to_string());
            i += op.len();
        } else if bytes[i] == b' ' || bytes[i] == b'\t' {
            end_word(&mut word, &mut tokens);
            i += 1;
        } else {
            let c = rest.chars().next().unwrap();
            word.push(c);
            i += c.len_utf8();
        }
    }
    end_word(&mut word, &mut tokens);

    let mut stages = Vec::new();
    let mut stage = StageSpec::default();
    let mut it = tokens.into_iter().peekable();
    while let Some(t) = it.next() {
        match t.as_str() {
            "|" => {
                if stage.words.is_empty() {
                    return Some(Err("syntax error near '|'".to_string()));
                }
                stages.push(core::mem::take(&mut stage));
            }
            "2>&1" => stage.errors_to_output = true,
            "<" | ">" | ">>" | "2>" | "2>>" => {
                let Some(file) = it.next_if(|w| !matches!(w.as_str(), "|" | "<" | ">" | ">>" | "2>" | "2>>" | "2>&1")) else {
                    return Some(Err(alloc::format!("'{}' needs a file name", t)));
                };
                match t.as_str() {
                    "<" => stage.input = Some(file),
                    ">" | ">>" => stage.output = Some((file, t == ">>")),
                    _ => stage.errors = Some((file, t == "2>>")),
                }
            }
            _ => stage.words.push(t),
        }
    }
    if stage.words.is_empty() {
        return Some(Err(if stages.is_empty() { "a redirection without a program" } else { "syntax error: nothing after '|'" }.to_string()));
    }
    stages.push(stage);
    Some(Ok(stages))
}

/// Runs `line` as a pipeline, if it is one: it has `|`, `<` or `>`, and
/// starts with a program (by name from /bin or /programs, by path, or after
/// `run`). Returns false for anything else, which is left to the other
/// commands: their own arguments may hold `<` and `>` (`play`, `kai`).
pub fn run_pipeline(line: &str) -> bool {
    let line = line.trim();
    let line = line.strip_prefix("run ").map(str::trim_start).unwrap_or(line);
    let first = line.split([' ', '\t', '|', '<', '>']).next().unwrap_or("");
    let is_program = |name: &str| if name.contains('.') || name.contains('/') { Some(name.to_string()) } else { find_command(name) };
    if is_program(first).is_none() {
        return false;
    }
    let stages = match parse_pipeline(line) {
        None => return false,
        Some(Err(e)) => {
            println!("{}", e);
            return true;
        }
        Some(Ok(stages)) => stages,
    };
    if let Err(e) = start_pipeline(stages, is_program) {
        println!("{}", e);
    }
    true
}

fn start_pipeline(stages: Vec<StageSpec>, is_program: impl Fn(&str) -> Option<String>) -> Result<(), String> {
    use crate::fd::{self, Fd, O_APPEND, O_CREAT, O_RDONLY, O_TRUNC, O_WRONLY};
    let cwd = current_cwd();
    let open_out = |(path, append): &(String, bool)| {
        fd::open(cwd.clone(), path, O_WRONLY | O_CREAT | if *append { O_APPEND } else { O_TRUNC })
    };
    // Check every program is there, and open every file, before starting any.
    let mut programs = Vec::new();
    for stage in &stages {
        let name = &stage.words[0];
        match is_program(name) {
            Some(path) => programs.push(path),
            None => return Err(alloc::format!("{}: not a program in /bin (shell commands can't be in a pipeline)", name)),
        }
    }
    let mut jobs = Vec::new();
    let mut next_input: Option<Fd> = None;
    for (i, (stage, path)) in stages.iter().zip(programs).enumerate() {
        let last = i + 1 == stages.len();
        let (pipe_read, pipe_write) = if last { (None, None) } else { let (r, w) = fd::pipe(); (Some(r), Some(w)) };
        let input = match &stage.input {
            Some(file) => fd::open(cwd.clone(), file, O_RDONLY)?,
            None => next_input.take().unwrap_or_else(fd::console),
        };
        let output = match &stage.output {
            Some(out) => open_out(out)?,
            None => pipe_write.unwrap_or_else(fd::console),
        };
        let errors = match &stage.errors {
            Some(err) => open_out(err)?,
            None if stage.errors_to_output => output.clone(),
            None => fd::console(),
        };
        next_input = pipe_read;
        jobs.push((path, stage.words[1..].join(" "), [input, output, errors]));
    }
    drop(next_input);
    crate::loader::run_pipeline(jobs);
    Ok(())
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;
    use alloc::vec;

    fn words(ws: &[&str]) -> Vec<String> {
        ws.iter().map(|w| w.to_string()).collect()
    }

    fn test_plain_commands_arent_pipelines() {
        assert_eq!(parse_pipeline("grep main notes.txt"), None);
    }

    fn test_pipeline_splits_at_bars() {
        let stages = parse_pipeline("ls /bin | grep sh|wc").unwrap().unwrap();
        assert_eq!(stages.len(), 3);
        assert_eq!(stages[0].words, words(&["ls", "/bin"]));
        assert_eq!(stages[1].words, words(&["grep", "sh"]));
        assert_eq!(stages[2].words, words(&["wc"]));
    }

    fn test_redirections_come_out() {
        let stages = parse_pipeline("wc <in.txt >> out.txt 2>&1").unwrap().unwrap();
        assert_eq!(stages, vec![StageSpec {
            words: words(&["wc"]),
            input: Some("in.txt".into()),
            output: Some(("out.txt".into(), true)),
            errors: None,
            errors_to_output: true,
        }]);
        let stages = parse_pipeline("grep a b 2> err > out").unwrap().unwrap();
        assert_eq!(stages[0].errors, Some(("err".into(), false)));
        assert_eq!(stages[0].output, Some(("out".into(), false)));
        assert_eq!(stages[0].words, words(&["grep", "a", "b"]));
    }

    fn test_a_2_inside_a_word_isnt_a_redirection() {
        let stages = parse_pipeline("echo a2>f").unwrap().unwrap();
        assert_eq!(stages[0].words, words(&["echo", "a2"]));
        assert_eq!(stages[0].output, Some(("f".into(), false)));
    }

    fn test_bad_pipelines() {
        assert!(parse_pipeline("| wc").unwrap().is_err());
        assert!(parse_pipeline("ls |").unwrap().is_err());
        assert!(parse_pipeline("ls >").unwrap().is_err());
        assert!(parse_pipeline("ls > | wc").unwrap().is_err());
        assert!(parse_pipeline("> out").unwrap().is_err());
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_plain_commands_arent_pipelines,
            &test_pipeline_splits_at_bars,
            &test_redirections_come_out,
            &test_a_2_inside_a_word_isnt_a_redirection,
            &test_bad_pipelines,
        ]);
    }
}
