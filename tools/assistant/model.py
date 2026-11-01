"""K-AI's model: a small transformer encoder with two heads.

Written out by hand instead of with nn.TransformerEncoderLayer, so every
operation has an obvious counterpart in KOS's Rust copy:

    token features --mean of embeddings--> + position --> 2 x block --> norm
        |-- mean over tokens --> intent head --> one score per intent
        '-- each token -------> tag head    --> one score per tag

    block(x) = x + attention(norm(x)), then x + feed_forward(norm(x))
    feed_forward(x) = W2 relu(W1 x + b1) + b2
"""

import math

import torch
from torch import nn

from kaitok import BUCKETS, MAX_TOKENS


class Config:
    def __init__(self, n_intents, n_tags, d=128, layers=2, heads=4, ff=512, dropout=0.1):
        self.n_intents, self.n_tags = n_intents, n_tags
        self.d, self.layers, self.heads, self.ff = d, layers, heads, ff
        self.dropout = dropout


class Block(nn.Module):
    def __init__(self, c):
        super().__init__()
        self.heads = c.heads
        self.norm1, self.norm2 = nn.LayerNorm(c.d), nn.LayerNorm(c.d)
        self.q, self.k, self.v, self.o = (nn.Linear(c.d, c.d) for _ in range(4))
        self.ff1, self.ff2 = nn.Linear(c.d, c.ff), nn.Linear(c.ff, c.d)
        # Only while training; it does nothing at inference.
        self.drop = nn.Dropout(c.dropout)

    def forward(self, x, pad):
        """x: [batch, tokens, d]; pad: [batch, tokens], True where padding."""
        b, t, d = x.shape
        h, hd = self.heads, d // self.heads
        y = self.norm1(x)
        split = lambda z: z.view(b, t, h, hd).transpose(1, 2)  # [b, h, t, hd]
        q, k, v = split(self.q(y)), split(self.k(y)), split(self.v(y))
        scores = q @ k.transpose(-1, -2) / math.sqrt(hd)  # [b, h, t, t]
        scores = scores.masked_fill(pad[:, None, None, :], float("-inf"))
        attended = (scores.softmax(-1) @ v).transpose(1, 2).reshape(b, t, d)
        x = x + self.drop(self.o(attended))
        return x + self.drop(self.ff2(torch.relu(self.ff1(self.norm2(x)))))


class KaiModel(nn.Module):
    def __init__(self, c):
        super().__init__()
        self.config = c
        # One extra row, never trained, pads tokens with fewer features.
        self.embed = nn.Embedding(BUCKETS + 1, c.d, padding_idx=BUCKETS)
        self.position = nn.Embedding(MAX_TOKENS, c.d)
        self.blocks = nn.ModuleList(Block(c) for _ in range(c.layers))
        self.norm = nn.LayerNorm(c.d)
        self.intent = nn.Linear(c.d, c.n_intents)
        self.tag = nn.Linear(c.d, c.n_tags)
        self.drop = nn.Dropout(c.dropout)

    def forward(self, features, counts, pad):
        """features: [batch, tokens, max features] bucket IDs (BUCKETS pads);
        counts: [batch, tokens] real features per token; pad: as in Block.
        Returns intent scores [batch, intents] and tag scores
        [batch, tokens, tags]."""
        x = self.embed(features).sum(2) / counts.clamp(min=1)[..., None]
        x = self.drop(x + self.position(torch.arange(x.shape[1], device=x.device)))
        for block in self.blocks:
            x = block(x, pad)
        x = self.norm(x)
        real = (~pad).float()[..., None]
        pooled = (x * real).sum(1) / real.sum(1)
        return self.intent(pooled), self.tag(x)
