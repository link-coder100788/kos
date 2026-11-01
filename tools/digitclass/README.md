# Digit recognition: training

Paint's "Read" button will recognise handwritten digits with a small
convolutional network trained here on your computer, the same way K-AI is
trained in `tools/assistant`. KOS runs its own Rust copy of the network.

## Files

| File | What it is |
|---|---|
| `prepare.py` | Turns a drawing into the network's input (KOS has an exact Rust copy). |
| `data.py` | Loads MNIST and redraws its digits as if with Paint's mouse, into `data/`. |
| `model.py` | The network: two convolutions with pooling, then two dense layers. |
| `train.py` | Trains it and reports accuracy on MNIST's test digits, plain and mouse-drawn. |
| `export.py` | Writes `disk/digits/digits.kosm` for KOS, and `golden.txt` to check KOS's copy against. |

## Setup

```sh
python3 -m venv .venv
.venv/bin/pip install torch numpy pillow pandas pyarrow huggingface_hub scikit-image
```

Or, from the top of the repo: `make digit-setup`.

MNIST goes in `data/mnist/` (not committed). To download it:

```sh
.venv/bin/python -c "from huggingface_hub import snapshot_download; snapshot_download('ylecun/mnist', repo_type='dataset', local_dir='data/mnist')"
```

## Training

```sh
.venv/bin/python data.py        # data/train.npz, data/test.npz (a few minutes)
.venv/bin/python train.py       # data/digits.pt, and an accuracy report
.venv/bin/python export.py      # ../../disk/digits/digits.kosm and golden.txt
```

Or, from the top of the repo: `make digit-train`.

## How a drawing becomes the network's input

MNIST digits are scanned pen strokes: cropped, about a tenth of their
height thick, shrunk into a 20×20 box and centred by centre of mass in
28×28. Paint's strokes are 4 px whatever the size of the digit, with hard
edges. `prepare.py` makes both look the same: crop to the ink, thicken thin
strokes to MNIST's proportions (`STROKE`, measured by `python prepare.py`),
shrink by averaging areas, centre. Training digits go through it too.

Most training digits are also redrawn the way Paint would draw them: blown
up, slanted, thinned to a skeleton and drawn over with a round brush. The
report's "mouse-drawn" line is how well the network reads those.
