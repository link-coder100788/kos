//! K-AI, KOS's built-in assistant. The `kai` crate understands what you
//! type (an intent and its slots, from a small model trained in
//! `tools/assistant`); this module loads that model from the disk and
//! connects it to the rest of the system.
//!
//! The model is loaded the first time it's needed and kept from then on.

pub mod tools;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use kai::digits::DigitModel;
use kai::Model;
use crate::arch::timer;
use crate::filesys::fs_ipc::{fs_call, FsRequest, FsResponse};
use crate::println;
use crate::std::lock::mutex::Mutex;

/// Where `make disk` / `make kai-upload` put the model.
const MODEL_PATH: &str = "/kai/kai.kosm";
/// Where `make disk` / `make digit-upload` put Paint's digit recogniser.
const DIGITS_PATH: &str = "/digits/digits.kosm";
/// Below this, K-AI asks what you meant instead of acting.
pub const CONFIDENT: f32 = 0.7;

static MODEL: Mutex<Option<Arc<Model>>> = Mutex::new(None);
static DIGITS: Mutex<Option<Arc<DigitModel>>> = Mutex::new(None);
/// A dangerous action waiting for yes or no. The shell sends the next line
/// you type to `answer` instead of running it as a command.
static PENDING: Mutex<Option<tools::Pending>> = Mutex::new(None);

/// Whether K-AI asked a yes/no question that hasn't been answered.
pub fn awaiting_answer() -> bool {
    PENDING.lock().is_some()
}

/// Your answer to K-AI's question. Only "y" or "yes" goes ahead; anything
/// else cancels, so a slip of the keyboard never deletes anything.
pub fn answer(line: &str) {
    let Some(action) = PENDING.lock().take() else { return };
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => println!("K-AI: {}", tools::run(action)),
        _ => println!("K-AI: OK, I won't."),
    }
}

/// The model, loading it from the disk on first use.
pub fn model() -> Result<Arc<Model>, String> {
    let mut slot = MODEL.lock();
    if let Some(model) = slot.as_ref() {
        return Ok(model.clone());
    }
    let started = timer::uptime_ms();
    let data = read_model(MODEL_PATH, "kai-upload")?;
    let model = Arc::new(Model::load(&data).map_err(|e| format!("{}: {}", MODEL_PATH, e))?);
    log::info!("kai: loaded {} ({} bytes, {} intents) in {} ms", MODEL_PATH, data.len(), model.intents.len(),
        timer::uptime_ms() - started);
    *slot = Some(model.clone());
    Ok(model)
}

/// Paint's digit recogniser, loading it from the disk on first use.
pub fn digits() -> Result<Arc<DigitModel>, String> {
    let mut slot = DIGITS.lock();
    if let Some(model) = slot.as_ref() {
        return Ok(model.clone());
    }
    let data = read_model(DIGITS_PATH, "digit-upload")?;
    let model = Arc::new(DigitModel::load(&data).map_err(|e| format!("{}: {}", DIGITS_PATH, e))?);
    log::info!("digits: loaded {} ({} bytes)", DIGITS_PATH, data.len());
    *slot = Some(model.clone());
    Ok(model)
}

/// A model file's bytes; the error says which `make` target puts it there.
fn read_model(path: &str, target: &str) -> Result<alloc::vec::Vec<u8>, String> {
    match fs_call(FsRequest::Read { cwd: String::from("/"), path: String::from(path) }) {
        FsResponse::Data(data) => Ok(data),
        FsResponse::Err(e) => Err(format!("can't read {}: {:?} (run `make {}`?)", path, e, target)),
        _ => Err(String::from("unexpected response from the filesystem")),
    }
}

/// The `kai` shell command. `kai --debug <text>` shows what the model made
/// of the text, without acting on it.
pub fn shell(args: &str) {
    let (debug, text) = match args.strip_prefix("--debug") {
        Some(rest) => (true, rest.trim()),
        None => (false, args.trim()),
    };
    // Empty input is a rule, not a guess: show what K-AI can do.
    if text.is_empty() {
        println!("K-AI: {}", tools::HELP);
        println!("(`kai --debug <request>` shows what I understood, without acting.)");
        return;
    }
    let model = match model() {
        Ok(model) => model,
        Err(e) => {
            println!("kai: {}", e);
            return;
        }
    };
    let started = timer::now();
    let u = model.understand(text);
    let micros = (timer::now() - started) * 1_000_000 / timer::frequency();
    if debug {
        println!("intent   {} ({:.2})   runner-up {} ({:.2})", u.intent, u.confidence, u.second.0, u.second.1);
        for (name, value) in &u.slots {
            println!("slot     {} = {:?}", name, value);
        }
        println!("took     {} us", micros);
        return;
    }
    if u.confidence < CONFIDENT {
        println!("K-AI: I'm not sure what you mean. Should I {}, or {}? Try saying it another way.",
            tools::describe(&u.intent), tools::describe(&u.second.0));
        return;
    }
    if tools::dangerous(&u.intent) && u.confidence < tools::DANGEROUS_CONFIDENT {
        println!("K-AI: I think you want me to {}, but I'm not sure enough to offer. Try saying it another way.",
            tools::describe(&u.intent));
        return;
    }
    match tools::act(&u, text) {
        tools::Outcome::Done(answer) => println!("K-AI: {}", answer),
        tools::Outcome::Confirm { question, action } => {
            println!("K-AI: {} (yes/no)", question);
            *PENDING.lock() = Some(action);
        }
    }
}
