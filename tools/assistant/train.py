"""Trains K-AI and reports how it does on the held-out data and tests.txt.

    .venv/bin/python train.py        # writes data/kai.pt
"""

import json
import random

import torch
from torch import nn

from check_tests import load as load_tests
from intents import INTENTS, INTENT_NAMES, TAGS
from kaitok import BUCKETS, MAX_TOKENS, features, parse_markup, token_tags, tokenize
from model import Config, KaiModel

INTENT_ID = {n: i for i, n in enumerate(INTENT_NAMES)}
TAG_ID = {n: i for i, n in enumerate(TAGS)}
# Scores below this make K-AI ask instead of act.
CONFIDENT = 0.7


def encode(token_lists):
    """Batch tensors for lists of tokens (cut to MAX_TOKENS)."""
    token_lists = [t[:MAX_TOKENS] for t in token_lists]
    t = max(len(toks) for toks in token_lists)
    feats = [[features(tok) for tok in toks] for toks in token_lists]
    f = max(len(x) for row in feats for x in row)
    ids = torch.full((len(feats), t, f), BUCKETS, dtype=torch.long)
    counts = torch.zeros(len(feats), t)
    pad = torch.ones(len(feats), t, dtype=torch.bool)
    for b, row in enumerate(feats):
        for i, x in enumerate(row):
            ids[b, i, :len(x)] = torch.tensor(x)
            counts[b, i] = len(x)
            pad[b, i] = False
    return ids, counts, pad


def slots_from_tags(text, tags):
    """{slot: value} from per-token tags, with values cut from `text` so
    they keep their original capitals."""
    spans = []
    for (_, start, end), tag in zip(tokenize(text), tags):
        if tag.startswith("B-") or (tag.startswith("I-") and (not spans or spans[-1][0] != tag[2:])):
            spans.append([tag[2:], start, end])
        elif tag.startswith("I-"):
            spans[-1][2] = end
    return {name: text[s:e] for name, s, e in spans}


@torch.no_grad()
def predict(model, texts):
    """(intent, confidence, slots) for each text."""
    model.eval()
    token_lists = [[t for t, _, _ in tokenize(x)] for x in texts]
    intents, tags = model(*encode(token_lists))
    probs = intents.softmax(-1)
    out = []
    for i, text in enumerate(texts):
        conf, best = probs[i].max(0)
        n = min(len(token_lists[i]), MAX_TOKENS)
        tag_names = [TAGS[j] for j in tags[i, :n].argmax(-1).tolist()]
        intent = INTENT_NAMES[best]
        # Like KOS: slots the chosen intent doesn't have are ignored.
        slots = {k: v for k, v in slots_from_tags(text, tag_names).items() if k in INTENTS[intent]}
        out.append((intent, conf.item(), slots))
    return out


def evaluate(model, examples, label, show_errors=False):
    """examples: (text, intent, slots). Prints intent and slot accuracy."""
    results = predict(model, [e[0] for e in examples])
    intent_ok = slot_ok = unsure = confidently_wrong = 0
    for (text, intent, slots), (got, conf, got_slots) in zip(examples, results):
        right = got == intent
        intent_ok += right
        slot_ok += right and got_slots == slots
        unsure += conf < CONFIDENT
        # The case that matters: K-AI would act on the wrong thing instead
        # of asking.
        confidently_wrong += not (right and got_slots == slots) and conf >= CONFIDENT
        if show_errors and not (right and got_slots == slots):
            print(f"    MISS {text!r}: wanted {intent} {slots}, got {got} {got_slots} ({conf:.2f})")
    n = len(examples)
    print(f"{label:9} intent {intent_ok / n:6.1%}   intent+slots {slot_ok / n:6.1%}   "
          f"unsure {unsure / n:5.1%}   confidently wrong {confidently_wrong / n:5.1%}   ({n} examples)")


@torch.no_grad()
def calibrate(model, examples):
    """Temperature scaling: finds the T that makes softmax(scores / T) best
    match how often the model is actually right on `examples`, then folds
    1/T into the intent head, so the exported model needs no extra step.
    An overconfident model gets T > 1, which softens its scores."""
    model.eval()
    token_lists = [[t for t, _, _ in tokenize(text)] for text, _, _ in examples]
    scores, _ = model(*encode(token_lists))
    labels = torch.tensor([INTENT_ID[intent] for _, intent, _ in examples])
    nll = lambda t: nn.functional.cross_entropy(scores / t, labels).item()
    best = min((x / 10 for x in range(5, 81)), key=nll)
    model.intent.weight /= best
    model.intent.bias /= best
    print(f"calibration: intent scores divided by {best:.1f} "
          f"(held-out loss {nll(1.0):.3f} -> {nll(best):.3f})")


def from_jsonl(path):
    rows = [json.loads(line) for line in open(path)]
    out = []
    for r in rows:
        out.append((r["text"], r["intent"], slots_from_tags(r["text"], r["tags"])))
    return rows, out


def main(epochs=12, seed=1):
    torch.manual_seed(seed)
    random.seed(seed)
    train_rows, _ = from_jsonl("data/train.jsonl")
    _, heldout = from_jsonl("data/heldout.jsonl")
    tests = []
    for _, intent, marked in load_tests("tests.txt"):
        text, spans = parse_markup(marked)
        if not tokenize(text):
            continue  # Empty input: KOS shows help without asking the model.
        tests.append((text, intent, {name: text[s:e] for name, s, e in spans}))

    model = KaiModel(Config(len(INTENT_NAMES), len(TAGS)))
    print(f"{sum(p.numel() for p in model.parameters()):,} parameters")
    opt = torch.optim.AdamW(model.parameters(), lr=1e-3, weight_decay=0.01)
    loss_fn = nn.CrossEntropyLoss()
    tag_loss_fn = nn.CrossEntropyLoss(ignore_index=-1)
    batch = 64
    for epoch in range(epochs):
        model.train()
        random.shuffle(train_rows)
        total = 0.0
        for i in range(0, len(train_rows), batch):
            rows = train_rows[i:i + batch]
            ids, counts, pad = encode([r["tokens"] for r in rows])
            intents = torch.tensor([INTENT_ID[r["intent"]] for r in rows])
            tags = torch.full(pad.shape, -1, dtype=torch.long)
            for b, r in enumerate(rows):
                for j, tag in enumerate(r["tags"][:MAX_TOKENS]):
                    tags[b, j] = TAG_ID[tag]
            intent_scores, tag_scores = model(ids, counts, pad)
            loss = loss_fn(intent_scores, intents) + tag_loss_fn(tag_scores.flatten(0, 1), tags.flatten())
            opt.zero_grad()
            loss.backward()
            opt.step()
            total += loss.item() * len(rows)
        print(f"epoch {epoch + 1:2}: loss {total / len(train_rows):.4f}")

    print()
    evaluate(model, heldout, "held-out")
    calibrate(model, heldout)
    evaluate(model, heldout, "held-out")
    evaluate(model, tests, "tests.txt", show_errors=True)
    torch.save(model.state_dict(), "data/kai.pt")


if __name__ == "__main__":
    main()
