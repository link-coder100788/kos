"""Generates K-AI's training data from templates.

Templates have interchangeable parts, `(a|b|c)`, each picked at random, so
one template stands for many phrasings. Slot placeholders ({host}, {site},
{path}, {topic}) are filled with values mostly *generated* at random (made-up host
names, URLs and paths), so the model learns where a detail sits in a
sentence rather than memorising particular values.

Templates marked with a leading "~" are held out: they never appear in
training, so accuracy on them shows whether the model handles phrasings it
hasn't seen.

    .venv/bin/python gen_data.py        # writes data/train.jsonl, data/heldout.jsonl
"""

import json
import random
import re
from pathlib import Path

from intents import INTENTS
from kaitok import parse_markup, token_tags

# Shared parts of phrasings.
CAN = "(can you|could you|would you|will you|pls|please|)"
SHOW = "(show|show me|list|display|give me|get|print|check|tell me|what's|whats|what is)"
MEM = "(memory|mem|ram|heap)"
NET = "(network|net|wifi|wi-fi|internet|connection|ethernet|networking)"
RUNNING = "(running|on|up|awake|alive|going|booted)"

TEMPLATES = {
    "help": [
        "help", "help me", "(help|helpp) (pls|please|me out)", "i need help", "what can you do",
        "what (can|do) you (do|know)", "how do i use (you|this|kai|k-ai)", "what are you (for|good for)",
        f"{SHOW} (your )?(commands|abilities|features|skills)", "what (can|should) i ask( you)?",
        "how does (this|kai|k-ai) work", "(commands|usage|instructions|manual)", "what are my options",
        "(explain|describe) (how (you|this|kai) works?|what you do|yourself)", "what (do you understand|questions can i ask)",
        "(show|give) me (some )?(examples|example questions|ideas)", "what (do you support|are you capable of)",
        "~what kinds of things can you do", "~give me some help", "~how can you help me",
        "~any tips on using you",
    ],
    "list_tasks": [
        "(tasks|processes|procs|threads|jobs)", f"{SHOW} (the )?(tasks|processes|procs|jobs)",
        "what('s|s| is) (running|going on|happening)( right now| rn| now)?", "everything (running|that's running)",
        "(which|what) (tasks|processes|programs) are (running|active|alive)",
        "what('s|s| is) (using|eating|hogging) (the|my) (cpu|processor)", "(which|what) task uses the most cpu",
        f"{SHOW} (the )?(busiest|top|running|active) (tasks|processes)", "what is the system doing",
        "(task|process) (list|manager)", "top", "what's busy",
        "what('s|s| is) (kos|the kernel|the system|the computer) (doing|up to|busy with)( right now| at the moment)?",
        "(which|what) task is (busy|slow|stuck|hanging)", f"{SHOW} (all )?the (kernel|scheduler)('s|s)? tasks",
        "~is anything running", "~what's keeping the cpu busy", "~running tasks", "~whats active",
    ],
    "memory_status": [
        f"{MEM}", f"{MEM} (stats|status|usage|use|info|left|free)", f"(current|free|used|available) {MEM}",
        f"{SHOW} (the )?{MEM}( usage| stats| status)?", f"how much {MEM} (is|do i have) (free|left|used|available)",
        f"how much {MEM} (am i|is the system) using", f"(am i|are we) (low on|out of|running out of) {MEM}",
        f"is {MEM} (running low|full|ok)", f"what's (my|the) {MEM} (at|usage|like)", f"check {MEM}",
        "~how full is memory", "~is there enough ram", "~mem left", "~how much space is left in memory",
    ],
    "uptime": [
        "uptime", "(up|awake|alive|run|running|boot) time", f"how long (have you|has (the system|kos|this|the computer)) been {RUNNING}",
        "how long (since|ago) (boot|you booted|startup|you started)", "when did (you|kos|the system) (boot|start|wake up)",
        f"{SHOW} (the )?uptime", "time since (boot|start|startup)", "how long (is|has) the uptime",
        "how long have we been (up|on|running)", "how (old|long) is (this|the current) (session|boot)",
        "how many (minutes|hours|seconds) have you been (up|running|on)", "when was the last (boot|restart)",
        "~how long ago did you start", "~been up long", "~how long since you started up", "~total uptime",
    ],
    "time": [
        "(time|date|clock|the time|the date)", "what('s|s| is) the (time|date|day)( today| now| right now)?",
        "what time is it( now| right now| rn)?", "(what|which) day is (it|today)", "what('s|s| is) (today's|todays) date",
        f"{CAN} tell me the (time|date|day)", "(show|give) me the (time|date|clock)", "(do you know|got) the time",
        "what (month|year|weekday) is it", "is it (morning|afternoon|evening|night)( yet)?", "how late is it",
        "what's the date and time", "current (time|date)", "time (please|pls|check)",
        "~what hour is it", "~tell me what time it is", "~date today", "~what day of the week is it", "~whats the time",
    ],
    "network_status": [
        f"{NET}", f"{NET} (status|stats|info|state|config|settings|details)", f"{SHOW} (the |my )?{NET}( status| info| config)?",
        "am i (connected|online)", f"is (the |my )?{NET} (up|working|ok|connected|configured)",
        "what('s|s| is) my (ip|ip address|address|gateway|dns|dns server)", "do i have (an ip|internet|a connection)",
        f"check (the |my )?{NET}", f"how('s|s| is) (the |my )?{NET}", "(dhcp|ip) (status|info|config)",
        "(did|has) dhcp (work|worked|finished)", "what (ip|address) did (dhcp|the router) give (me|us|kos)",
        f"{SHOW} (the |my )?(gateway|dns|dns servers|router)( and (dns|gateway|ip))?",
        "~what network am i on", "~do i have a network connection", "~tell me about the network", "~ip config",
    ],
    "ping": [
        "ping {host}", "(net|network) ping {host}", f"{CAN} ping {{host}}", "(try|go) ping(ing)? {host}",
        "is {host} (up|down|online|reachable|alive|responding)", "can (you|kos|i|we) (reach|ping|contact) {host}",
        "(check|test|see) (if|whether) {host} (is up|is reachable|responds|answers)", "send a ping to {host}",
        "(check|test) (the )?(connection|latency) to {host}", "how fast is {host}", "(what's|whats) the ping (to|for) {host}",
        "(latency|round trip|rtt|ping time) (to|for) {host}", "does {host} (answer|reply to|respond to) pings?",
        "(test|probe) {host}", "is {host} down or (is it me|just me)",
        "~does {host} respond", "~is {host} there", "~ping {host} for me real quick", "~reach {host}",
    ],
    "open_url": [
        "(open|load|visit|browse|launch|search) {site}", "(go to|take me to|navigate to|bring up|pull up|head to) {site}",
        f"{CAN} (open|load|show me) {{site}}", "(open|load|show me) the {site} (website|site|page|homepage)",
        "(open|load) {site} in the browser", "i (want|wanna|would like) to (see|visit|go to) {site}",
        "(search|look up|search for) {site}", "show me {site}", "browse to {site}", "web {site}",
        "i (want|wanna|would like) to (read|check out|look at) {site}", "(check out|read) {site}",
        "~let's go to {site}", "~open up {site}", "~i'd like to visit {site}", "~get me to {site}",
    ],
    "list_files": [
        "(ls|dir|files)", "ls {path}", "(list|show|display) (the )?(files|folder|directory|dir)",
        "(list|show|ls|dir) {path}", "(what's|whats|what is) in {path}", f"{SHOW} (the )?(files|contents) (in|of) {{path}}",
        "(what|which) files are (in|inside) {path}", "(what's|whats) (inside|in) (this|the current) (folder|directory|dir)",
        "(list|show) (the )?(files|contents) (in|of|inside) {path}", "what do i have in {path}", "open (the )?folder {path}",
        "~show what's in {path}", "~what is in the {path} folder", "~can you list {path}", "~let me see the files",
        "~peek inside {path}",
    ],
    "delete_file": [
        "(delete|remove|erase|rm|del|trash) {path}", "(delete|remove|erase) (the )?(file|folder|directory) {path}",
        f"{CAN} (delete|remove|get rid of) {{path}}", "(get rid of|wipe|throw away|bin) {path}",
        "i (want|need|would like) to (delete|remove) {path}", "{path} (can go|is not needed|is junk), (delete|remove) it",
        "(delete|remove) {path} (from the disk|for good|permanently)", "(delete|remove) (a|the|that|this) file",
        "(i don't|i dont|i do not) (need|want|use) {path}( anymore| any more| now)?(,)? (delete|remove|trash) it",
        "{path} is (old|junk|useless|not needed)( now| anymore)?(,)? (delete|remove) it",
        "~please erase {path}", "~zap {path}", "~make {path} go away", "~kill the file {path}",
    ],
    "diagnose": [
        "what went wrong", "what('s|s| is) (wrong|broken|going wrong)", "why did (that|it|this) (fail|break|crash|not work)",
        "(any|were there any|are there any|got any) (errors|problems|warnings|issues|failures)( lately| so far| today)?",
        f"{SHOW} (the )?(errors|problems|warnings|recent errors|error log)", "(check|scan|read|look at) the (logs|log|kernel log|errors|dmesg)",
        "(explain|summarize|summarise) the (errors|logs|log|warnings|dmesg)", "(did|has) anything (go wrong|gone wrong|fail|failed|break|broken)",
        "something('s|s| is) (wrong|broken|off|not working)",
        "(any )?(errors|problems|warnings|failures) since (boot|startup|you started|i asked|last time)",
        "(did|has) anything (fail|go wrong|break) (just now|recently|lately|earlier)", "(diagnose|debug|troubleshoot) (the problem|this|it|kos|the system)",
        "what went wrong with {topic}", "what('s|s| is) wrong with {topic}", "why (is|isn't|isnt|is not) {topic} (working|broken|failing)",
        "why (did|does|do) {topic} (fail|keep failing|not work|break)", "(diagnose|debug|troubleshoot) {topic}",
        "{topic} (isn't|isnt|is not|doesn't|doesnt|does not|won't|wont|stopped) work(ing)?", "{topic} (is broken|failed|keeps failing)",
        "(any|were there any|are there any) (errors|problems|warnings|issues) (with|in|from) {topic}",
        "(show|check) (the )?{topic} (errors|problems|warnings|logs)", "what happened (to|with) {topic}",
        "~why did that not work", "~what broke", "~what's up with {topic}", "~find out why {topic} fails",
        "~any trouble with {topic}", "~what errors happened",
    ],
    "unknown": [
        # Things K-AI can't do (yet), including near misses for future intents.
        "(what's|whats|what is) {num} (times|plus|minus|divided by) {num}",
        "(you're|youre|you are) (awesome|great|cool|dumb|funny)", "what (language|code) are you (written|made|built) in",
        "(make|create) a (folder|directory) (called|named) {path}",
        "(write|save) (hello|some text|this) to {path}", "(rename|move|copy) {path} to (backup|old|somewhere)",
        "(shut down|reboot|restart|turn off|power off)( the computer| kos| now)?", "(close|quit|exit) the (browser|terminal|window)",
        "(play|stop) (some )?(music|a song|the radio)", "set (an alarm|a timer|a reminder)", "what's the weather( like)?( in paris| tomorrow| today)?",
        "tell me a (joke|story)", "write (a poem|an essay|some code)", "what is (2 plus 2|the square root of 9|5 times 7)",
        "(what's|what is) the capital of (france|japan|peru)", "who (invented|made|created) (unix|linux|the internet)",
        "translate (hello|this) (to|into) (french|spanish)", "how do i (cook rice|tie a tie|learn rust)",
        "(set|change|fix) the (clock|time zone|timezone|date)", "(start|set) a (stopwatch|countdown)",
        "(how are you|how's it going|how you doing|what's up|sup|good morning|hello|hi|hey|thanks|thank you)",
        "(who are you|what's your name|are you human|do you dream|what's your favorite color)",
        "(change|set) the (wallpaper|theme|font|volume|brightness)", "(install|download|update) (a program|a file|kos)",
        "(send an email|call mom|order a pizza|book a flight)", "what's the meaning of life", "(banana|asdf|hmm|ok|lol|yes|no|maybe)",
        "the quick brown fox", "i like (turtles|trains|pizza)", "why is the sky blue",
        "~format the disk", "~empty the trash", "~what's the news today", "~sing a song", "~remind me at noon",
        "~how tall is mount everest", "~increase the volume", "~lock the screen", "~good night", "~you're cool",
    ],
}

# A few real values, and pieces to generate the rest from.
REAL_HOSTS = ["example.com", "google.com", "wikipedia.org", "debian.org", "github.com", "wiby.me", "10.0.2.2",
              "1.1.1.1", "8.8.8.8", "kernel.org", "news.ycombinator.com", "localhost", "127.0.0.1", "router"]
REAL_SITES = ["wikipedia", "google", "debian", "github", "wiby", "hacker news", "osdev wiki", "youtube", "reddit",
              "the rust website", "duckduckgo", "https://en.wikipedia.org/wiki/Kernel_(operating_system)"]
REAL_TOPICS = ["the network", "network", "tls", "https", "the disk", "the filesystem", "memory", "dns", "dhcp",
               "the browser", "the screen", "the mouse", "certificates", "wifi", "the internet", "my program", "files"]
REAL_PATHS = ["/", "/files", "files", "/files/files2/images", "..", ".", "docs", "images", "programs", "/bin"]
WORDS = """apple river stone cloud pixel orbit cedar maple delta echo falcon garden harbor island jungle
kettle lemon meadow north ocean pepper quartz robin salmon tiger umbrella violet walnut yellow zebra amber
blue coral dusk ember frost glow honey iris jade kite lunar mint nova olive pine quill rust sage tulip vale
willow arch byte core data drive forge grid hub kern link mesh node port queue root shell stack tree unit
vault web zone alpha beta gamma photo music video notes backup report draft config logs project build src
tmp home user work old new test demo blog shop mail news wiki docs code dev labs cloud app""".split()
TLDS = ["com", "org", "net", "io", "dev", "me", "local", "social", "co.uk", "edu", "info", "xyz"]
EXTS = ["txt", "png", "jpg", "bmp", "md", "rs", "c", "elf", "log", "html"]


def random_host(rng):
    kind = rng.random()
    if kind < 0.25:
        return ".".join(str(rng.randrange(1, 255)) for _ in range(4))
    if kind < 0.35:
        return rng.choice(WORDS)
    name = rng.choice(WORDS) + rng.choice(["", "", "-" + rng.choice(WORDS), str(rng.randrange(10))])
    sub = rng.choice(["", "", "www.", "mail.", "en.", rng.choice(WORDS) + "."])
    return f"{sub}{name}.{rng.choice(TLDS)}"


def random_path(rng):
    parts = [rng.choice(WORDS) for _ in range(rng.choice([1, 1, 2, 2, 3]))]
    if rng.random() < 0.3:
        parts[-1] += "." + rng.choice(EXTS)
    lead = rng.choice(["", "/", "/", "./", "../"])
    return lead + "/".join(parts)


def random_site(rng):
    kind = rng.random()
    if kind < 0.35:
        return random_host(rng)
    if kind < 0.6:
        path = rng.choice(["", "", random_path(rng).lstrip("./")])
        return f"{rng.choice(['https', 'http'])}://{random_host(rng)}/{path}"
    if kind < 0.8:
        return rng.choice(WORDS)
    return rng.choice([f"the {rng.choice(WORDS)} (docs|wiki|website|site|homepage)", f"{rng.choice(WORDS)} (news|wiki|docs)",
                       f"the {rng.choice(WORDS)} {rng.choice(WORDS)} page"])


def made_up_word(rng):
    """A pronounceable word that's in no list, like "tovaril"."""
    return "".join(rng.choice("bcdfghklmnprstvwz") + rng.choice("aeiou") for _ in range(rng.choice([1, 2, 2, 3]))) \
        + rng.choice(["", "", "k", "n", "s", "t", "l"])


def random_topic(rng):
    # Half made-up words, so the model learns where a topic sits in a
    # sentence instead of memorising which words can be one.
    word = lambda: made_up_word(rng) if rng.random() < 0.5 else rng.choice(WORDS)
    words = " ".join(word() for _ in range(rng.choice([1, 1, 2, 2, 3])))
    lead = rng.choice(["", "", "the ", "the ", "my "])
    tail = rng.choice(["", "", "", " driver", " app", " program", " task", " stack", " loading", " downloads", " device"])
    return lead + words + tail


FILLERS = {
    "host": (REAL_HOSTS, random_host),
    "site": (REAL_SITES, random_site),
    "path": (REAL_PATHS, random_path),
    "topic": (REAL_TOPICS, random_topic),
}

# Words that can follow a detail, so the model learns where it ends.
AFTER_SLOT = ["", "", "", "", " real quick", " from here", " right now", " for me", " in the browser",
              " please", " or is it me", " again", " now", " if you can", " thanks"]

PREFIXES = ["", "", "", "", "", "kai ", "kai, ", "hey kai, ", "k-ai, ", "yo ", "ok ", "um ", "so "]
SUFFIXES = ["", "", "", "", "?", "??", " please", " pls", " for me", ".", " now", "!", " rn", " thanks"]
ALTERNATIVES = re.compile(r"\(([^()]*)\)(\?)?")


def choose(template, rng):
    """Picks one of each `(a|b|c)` group; `(a)?` means a or nothing."""
    def pick(m):
        options = m.group(1).split("|")
        if m.group(2):
            options.append("")
        return rng.choice(options)
    while ALTERNATIVES.search(template):
        template = ALTERNATIVES.sub(pick, template, count=1)
    return re.sub(r" +", " ", template).strip()


def typo(word, rng):
    """A plausible slip of the keyboard: drop, double or swap a letter."""
    if len(word) < 4:
        return word
    i = rng.randrange(1, len(word) - 1)
    kind = rng.randrange(3)
    if kind == 0:
        return word[:i] + word[i + 1:]
    if kind == 1:
        return word[:i] + word[i] + word[i:]
    return word[:i - 1] + word[i] + word[i - 1] + word[i + 1:]


def example(intent, template, rng):
    marked = choose(template, rng)
    while "{num}" in marked:
        marked = marked.replace("{num}", str(rng.randrange(100)), 1)
    if template.rstrip().endswith("}") and intent != "unknown":
        marked += rng.choice(AFTER_SLOT)
    for name, (real, generate) in FILLERS.items():
        while "{" + name + "}" in marked:
            value = rng.choice(real) if rng.random() < 0.3 else choose(generate(rng), rng)
            marked = marked.replace("{" + name + "}", f"[{name}: {value}]", 1)
    marked = rng.choice(PREFIXES) + marked + rng.choice(SUFFIXES)
    if rng.random() < 0.25:
        words = marked.split(" ")
        i = rng.randrange(len(words))
        if "[" not in words[i] and "]" not in words[i] and ":" not in words[i]:
            words[i] = typo(words[i], rng)
        marked = " ".join(words)
    if rng.random() < 0.3:
        marked = marked[:1].upper() + marked[1:]
    text, spans = parse_markup(marked)
    # In `unknown` examples the paths are just words.
    tokens, tags = token_tags(text, [] if intent == "unknown" else spans)
    return {"text": text, "intent": intent, "tokens": tokens, "tags": tags}


def main(per_template=80, seed=1):
    rng = random.Random(seed)
    assert set(TEMPLATES) == set(INTENTS), "every intent needs templates"
    train, heldout = [], []
    for intent, templates in TEMPLATES.items():
        for template in templates:
            held = template.startswith("~")
            template = template.lstrip("~")
            # Fewer copies of short fixed phrases, which would otherwise
            # just repeat.
            n = per_template if ("{" in template or "(" in template) else per_template // 4
            for _ in range(n):
                (heldout if held else train).append(example(intent, template, rng))
    rng.shuffle(train)
    Path("data").mkdir(exist_ok=True)
    for name, rows in [("train", train), ("heldout", heldout)]:
        with open(f"data/{name}.jsonl", "w") as f:
            for row in rows:
                f.write(json.dumps(row) + "\n")
    print(f"{len(train)} training and {len(heldout)} held-out examples")
    counts = {i: sum(r["intent"] == i for r in train) for i in INTENTS}
    print("training examples per intent:", counts)


if __name__ == "__main__":
    main()
