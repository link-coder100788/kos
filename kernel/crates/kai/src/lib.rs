//! K-AI: KOS's built-in assistant, understanding side. A small model
//! trained in `tools/assistant` reads what you type and picks an *intent*
//! (what you want) and *slots* (the details); KOS acts on them.
//!
//! `no_std` + `alloc`, so the kernel can use it; tests run on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod digits;
pub mod model;
pub mod tokenizer;

use alloc::string::String;
use alloc::vec::Vec;
pub use model::Model;

/// What K-AI understood from one text.
pub struct Understanding {
    /// The most likely intent, e.g. "ping".
    pub intent: String,
    /// How sure the model is of it, 0 to 1.
    pub confidence: f32,
    /// The runner-up, for "did you mean...?".
    pub second: (String, f32),
    /// Slots as (name, value). Values are cut from the original text, so
    /// they keep their capitals. Not filtered by intent: a tool reads only
    /// the slots it wants.
    pub slots: Vec<(String, String)>,
}

impl Model {
    /// Runs the model and decodes its scores into an intent and slots.
    pub fn understand(&self, text: &str) -> Understanding {
        let out = self.run(text);
        let mut probs = out.intent_scores.clone();
        model::softmax(&mut probs);
        let mut order: Vec<usize> = (0..probs.len()).collect();
        order.sort_by(|&a, &b| probs[b].total_cmp(&probs[a]));
        let (best, second) = (order[0], order.get(1).copied().unwrap_or(order[0]));

        // B-x starts a slot; I-x continues one (or starts it, if the model
        // skipped the B-).
        let mut slots: Vec<(String, usize, usize)> = Vec::new();
        for (token, scores) in out.tokens.iter().zip(&out.tag_scores) {
            let tag = &self.tags[argmax(scores)];
            let continues = tag.strip_prefix("I-").is_some_and(|n| slots.last().is_some_and(|s| s.0 == n));
            if continues {
                slots.last_mut().unwrap().2 = token.end;
            } else if let Some(name) = tag.strip_prefix("B-").or_else(|| tag.strip_prefix("I-")) {
                slots.push((String::from(name), token.start, token.end));
            }
        }
        Understanding {
            intent: self.intents[best].clone(),
            confidence: probs[best],
            second: (self.intents[second].clone(), probs[second]),
            slots: slots.into_iter().map(|(name, s, e)| (name, String::from(&text[s..e]))).collect(),
        }
    }
}

fn argmax(x: &[f32]) -> usize {
    (0..x.len()).max_by(|&a, &b| x[a].total_cmp(&x[b])).unwrap_or(0)
}
