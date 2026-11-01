"""The digit recogniser: a small convolutional network.

    [1, 28, 28] -> conv 3x3 (16) -> relu -> max pool 2x2    [16, 14, 14]
                -> conv 3x3 (32) -> relu -> max pool 2x2    [32, 7, 7]
                -> dense (128) -> relu -> dense (10)        one score per digit

Convolutions are padded by 1, so they keep the size. Every operation has an
obvious counterpart in KOS's Rust copy.
"""

from torch import nn


class Config:
    def __init__(self, c1=16, c2=32, hidden=128, classes=10, dropout=0.3):
        self.c1, self.c2, self.hidden, self.classes = c1, c2, hidden, classes
        self.dropout = dropout


class DigitModel(nn.Module):
    def __init__(self, c):
        super().__init__()
        self.conv1 = nn.Conv2d(1, c.c1, 3, padding=1)
        self.conv2 = nn.Conv2d(c.c1, c.c2, 3, padding=1)
        self.pool = nn.MaxPool2d(2)
        self.fc1 = nn.Linear(c.c2 * 7 * 7, c.hidden)
        self.fc2 = nn.Linear(c.hidden, c.classes)
        # Only while training; it does nothing at inference.
        self.drop = nn.Dropout(c.dropout)

    def forward(self, x):
        """x: [batch, 1, 28, 28] in [0, 1]. Returns [batch, classes] scores."""
        x = self.pool(self.conv1(x).relu())
        x = self.pool(self.conv2(x).relu())
        x = self.drop(self.fc1(x.flatten(1)).relu())
        return self.fc2(x)
