# K-AI: training

K-AI is KOS's built-in assistant. A small model, trained here on your
computer, reads what you type and picks an *intent* (what you want, such as
`ping`) and its *slots* (the details, such as the host). KOS then does it
with its own functions and answers from live system data.

## Files

| File | What it is |
|---|---|
| `intents.py` | The intents and their slots. The single source of truth. |
| `kaitok.py` | The tokenizer and hashed token features (KOS has an exact Rust copy). |
| `gen_data.py` | Generates training data from templates into `data/`. |
| `tests.txt` | Your own test sentences, never trained on. |
| `check_tests.py` | Checks `tests.txt` for mistakes. |
| `model.py` | The model: a small transformer with an intent head and a slot-tag head. |
| `train.py` | Trains it and reports accuracy on held-out data and `tests.txt`. |
| `export.py` | Writes `disk/kai/kai.kosm` for KOS, and `golden.txt` to check KOS's copy against. |

## Setup

```sh
python3 -m venv .venv
.venv/bin/pip install torch numpy
```

## Training

```sh
.venv/bin/python gen_data.py    # data/train.jsonl, data/heldout.jsonl
.venv/bin/python train.py       # data/kai.pt, and an accuracy report
.venv/bin/python export.py      # ../../disk/kai/kai.kosm and golden.txt
```

Or, from the top of the repo:

```sh
make kai-setup     # once: the venv and PyTorch
make kai-train     # check tests.txt, generate data, train, export
make kai-upload    # put the model on disk.img, keeping files written in KOS
make kai           # kai-train, then kai-upload
```

The report gives, for the held-out templates and for `tests.txt`, how often
the intent is right, how often the intent *and* every slot are right, how
often the model is unsure (below 0.7, so K-AI asks), and how often it's
**confidently wrong**, which is the number to watch: those are the times
K-AI would act on a misunderstanding.

## tests.txt

One test per line, `intent | what you'd type`, with slots marked
`[slot: value]`. Blank lines and `#` comments are ignored.

```
ping       | is [host: debian.org] up?
open_url   | take me to [site: wikipedia]
list_files | what's in [path: /files/images]
unknown    | what's the weather like
```

A line can also be a pattern with `<slot>` where a value goes, like
`ping | is <host> up`. It's tested with a few values that aren't in the
training data (`<url>` means `<site>`).

Write them the way *you* would actually type, typos and all. Check them with
`.venv/bin/python check_tests.py`.
