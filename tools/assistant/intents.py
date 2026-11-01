"""K-AI's intents and slots: the single source of truth.

The exported model file carries these names, so KOS reads them from the
model rather than keeping its own copy. Add an intent here, give it
templates in gen_data.py, retrain, and KOS sees it (it still needs a tool
to act on it).
"""

# Intent name -> the slots it can have. `unknown` must exist: it's what the
# model answers for anything it can't do.
INTENTS = {
    "help": [],
    "list_tasks": [],
    "memory_status": [],
    "uptime": [],
    "time": [],
    "network_status": [],
    "ping": ["host"],
    "open_url": ["site"],
    "list_files": ["path"],
    "diagnose": ["topic"],
    # Dangerous: KOS always asks before acting on it (see tools.rs).
    "delete_file": ["path"],
    "unknown": [],
}

INTENT_NAMES = list(INTENTS)
SLOT_NAMES = sorted({s for slots in INTENTS.values() for s in slots})

# Per-token tags: O (not part of a slot), then B- (first token) and I-
# (following tokens) for each slot.
TAGS = ["O"] + [f"{p}-{s}" for s in SLOT_NAMES for p in ("B", "I")]
