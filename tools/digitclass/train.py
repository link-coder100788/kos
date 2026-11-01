"""Trains the digit recogniser and reports how well it does.

    .venv/bin/python train.py       # data/digits.pt, and an accuracy report

The report gives, for MNIST's test digits as they are and redrawn as if
with the mouse, how often the digit is right, how often the model is unsure
(below CONFIDENT, so Paint shows "?"), and how often it's **confidently
wrong**: the number to watch.
"""

import random
from collections import Counter

import numpy as np
import torch
from torch import nn

from model import Config, DigitModel

# Below this, Paint shows "?" instead of a digit.
CONFIDENT = 0.7
# Training digits kept back to calibrate the scores on.
CALIBRATION = 5000


def device():
    return torch.device("mps" if torch.backends.mps.is_available() else "cpu")


def tensor(x):
    """uint8 [n, 28, 28] -> float [n, 1, 28, 28] in [0, 1]."""
    return torch.from_numpy(x).float().div(255).unsqueeze(1)


def shift(x):
    """Each digit moved by up to a pixel: where the centre of mass lands is
    rounded, so KOS's copy can be a pixel off from the training digits."""
    dy, dx = random.randint(-1, 1), random.randint(-1, 1)
    return torch.roll(x, (dy, dx), (2, 3))


@torch.no_grad()
def scores(model, x, batch=2000):
    model.eval()
    dev = next(model.parameters()).device
    return torch.cat([model(x[i:i + batch].to(dev)).cpu() for i in range(0, len(x), batch)])


def evaluate(model, x, y, name):
    probs = scores(model, x).softmax(1)
    conf, pred = probs.max(1)
    right = pred == y
    sure = conf >= CONFIDENT
    print(f"{name:<12} right {right.float().mean() * 100:5.2f}%   unsure {(~sure).float().mean() * 100:5.2f}%   "
          f"confidently wrong {(sure & ~right).float().mean() * 100:5.2f}%   ({len(y):,} digits)")
    mistakes = Counter((int(a), int(b)) for a, b in zip(y[~right], pred[~right]))
    print("    most confused (digit -> read as):", ", ".join(f"{a}->{b} x{n}" for (a, b), n in mistakes.most_common(6)))


def calibrate(model, x, y):
    """Divides the final scores by the temperature that makes the held-out
    probabilities most honest, so "90% sure" is right about 90% of the time."""
    raw = scores(model, x)
    nll = lambda t: nn.functional.cross_entropy(raw / t, y).item()
    best = min((t / 10 for t in range(5, 51)), key=nll)
    with torch.no_grad():
        model.fc2.weight /= best
        model.fc2.bias /= best
    print(f"calibration: scores divided by {best:.1f} (held-out loss {nll(1.0):.4f} -> {nll(best):.4f})")


def main(epochs=10, seed=1, batch=128):
    torch.manual_seed(seed)
    random.seed(seed)
    train = np.load("data/train.npz")
    test = np.load("data/test.npz")
    x, y = tensor(train["x"]), torch.from_numpy(train["y"]).long()
    order = torch.randperm(len(x))
    held, used = order[:CALIBRATION], order[CALIBRATION:]
    x_held, y_held, x, y = x[held], y[held], x[used], y[used]

    model = DigitModel(Config()).to(device())
    print(f"{sum(p.numel() for p in model.parameters()):,} parameters, training on {device()}")
    opt = torch.optim.AdamW(model.parameters(), lr=2e-3, weight_decay=1e-4)
    steps = epochs * ((len(x) + batch - 1) // batch)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, max_lr=2e-3, total_steps=steps)
    loss_fn = nn.CrossEntropyLoss()
    for epoch in range(epochs):
        model.train()
        perm = torch.randperm(len(x))
        total = 0.0
        for i in range(0, len(x), batch):
            idx = perm[i:i + batch]
            xb, yb = shift(x[idx]).to(device()), y[idx].to(device())
            loss = loss_fn(model(xb), yb)
            opt.zero_grad()
            loss.backward()
            opt.step()
            sched.step()
            total += loss.item() * len(idx)
        print(f"epoch {epoch + 1:2}: loss {total / len(x):.4f}")

    print()
    calibrate(model, x_held, y_held)
    model.cpu()
    y_test = torch.from_numpy(test["y"]).long()
    evaluate(model, tensor(test["plain"]), y_test, "MNIST")
    evaluate(model, tensor(test["mouse"]), y_test, "mouse-drawn")
    torch.save(model.state_dict(), "data/digits.pt")


if __name__ == "__main__":
    main()
