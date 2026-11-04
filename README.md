# KOS

A hobby operating system for 64-bit ARM (AArch64), written in Rust and run on QEMU's `virt` machine.

It boots on five cores into a graphical desktop with overlapping windows, a real keyboard and mouse, a terminal, networking with HTTPS and a web browser that renders HTML and images, a FAT file system on a virtual disk, one-voice sound (tones, tunes and WAV files, TempleOS style), C programs, a compiler for its own little language, K, that runs inside the kernel, and K-AI, a built-in assistant powered by a small neural network trained for KOS.

## Quick start

On macOS with [Homebrew](https://brew.sh):

```sh
brew install qemu lld                     # QEMU, plus the linker for C programs
curl https://sh.rustup.rs -sSf | sh       # Rust, if you don't have it
cargo install cargo-binutils              # provides rust-objcopy

git clone https://github.com/link-coder100788/vm_os.git
cd vm_os
make run-gui
```

The first build downloads the pinned nightly Rust toolchain (see `rust-toolchain.toml`) and creates `disk.img`. A QEMU window opens with the KOS desktop, and the same shell is also available in the terminal you ran `make` from.

On Linux, install `qemu-system-aarch64` (often in a `qemu-system-arm` package), `clang` and `lld`, then follow the same steps. `make run-gui` uses QEMU's GTK display there; pass `GUI=sdl` to use SDL instead.

**Quitting:** close the QEMU window, or press `Ctrl-A` then `X` in the terminal.

## Running

```sh
make run-gui    # the desktop in a QEMU window, plus the serial console in this terminal
make run        # serial console only, no display
make test       # run the in-kernel test suite; exits 0 if every test passed
make disk       # rebuild disk.img from scratch (discards changes made from inside KOS)
make user       # just build the userspace programs into build/user/
make kai        # retrain K-AI and put the new model on disk.img (see "K-AI" below)
make            # just build build/kos.bin
make clean
```

Options are passed as make variables:

| Variable   | Default           | Meaning                                                                  |
|------------|-------------------|--------------------------------------------------------------------------|
| `SMP`      | `5`               | CPU count. Core 0 runs the kernel's tasks, core 1 threads, core 2 async tasks, and cores 3-4 programs. Fewer cores works; whatever's missing runs on core 0. |
| `MEM`      | `512M`            | RAM. The memory map in `kernel/src/mm/layout.rs` assumes 512 MiB.        |
| `GUI`      | `cocoa` / `gtk`, zoom-to-fit | QEMU display backend for `run-gui`. Zoom-to-fit scales KOS to the window: resize it or go full screen. |
| `LOG`      | (empty)           | Log levels at boot, e.g. `LOG=debug` or `LOG="net=trace,console=debug"`. See [Logging](#logging-and-debugging). |
| `FEATURES` | (empty)           | Kernel cargo features: `test` (in-kernel test suite), `restart_secondary` (restart a core that panics). |
| `DISK`     | `disk.img`        | Disk image to attach.                                                    |
| `UTC_OFFSET` | this computer's (`date +%z`) | Time zone for the taskbar clock, e.g. `+0100`. The clock chip itself runs in UTC. |
| `AUDIO`    | `coreaudio` / `pa` | QEMU audio backend for the sound device: your speakers on macOS / Linux. `AUDIO=none` for silence, `AUDIO="wav,path=out.wav"` to record. `make test` always records to `build/test.wav`. |

## Using KOS

### The desktop

KOS boots into a 1024x768 desktop with a Terminal open.

- Click **KOS** in the taskbar for the menu: **Terminal**, **Browser**, **Programs**, **Task Manager**, **Paint**, **About KOS**.
- Windows overlap: click one to raise it, drag it by the title bar, close it with the red box. The blue box next to it rolls the window up to just its title bar; click it again, or the window's taskbar button, to unroll it. The taskbar has a button per window and the date and local time. Most windows (Terminal, Browser, Task Manager, Programs, Paint, images and videos) resize: drag any edge or corner, or maximize with the box between the other two, or by double-clicking the title bar. Dragging a maximized window by its title bar brings it back to its old size.
- The keyboard goes to the focused window.
- **Programs** lists the programs on the disk; click one to run it. Its output appears in the Terminal.
- **Task Manager** lists every task: programs first, with how they were started, their CPU use over the last second and their heap, then the kernel's own tasks. Select a program and click **End task** (or press Delete) to kill it; its windows close with it.
- **Browser** renders HTML pages with [lighthouse](kernel/crates/lighthouse), a small HTML/CSS engine, including PNG, JPEG and BMP images. Click links to follow them (the status line shows where they go), and use **<** / **>** for back and forward. Form fields take clicks and typing, and Enter or a submit button sends the form (GET only). **Source** or Ctrl+U shows the raw response, and Ctrl+L goes to the address bar. Scroll with the wheel, arrow keys, Page Up/Down or Home/End. Both `http://` and `https://` work; https uses TLS 1.3 but doesn't check the server's certificate yet. There's no JavaScript, so sites that need it (like Google search) won't work.
- **About KOS** shows live uptime, cores, memory use and how idle the CPU is.
- `open picture.png` in the shell opens a PNG, JPEG or BMP image in a window. Images bigger than the window scroll with the wheel, arrow keys, Page Up/Down and Home/End, or by dragging.

The mouse and keyboard are virtio-input devices. The mouse is an absolute tablet, so the pointer just follows your cursor over the QEMU window.

### The shell

The shell runs on the serial console and in the Terminal window at the same time; type in either. Ctrl+C ends the program started with `run`, or at the prompt throws away the line you're typing.

| Command | What it does |
|---------|--------------|
| `help` | List commands |
| `ls`, `cd`, `cat`, `mkdir`, `rm` `<path>` | Files |
| `write <path> <text>` | Create a file |
| `run <path>.elf.b64` | Run a program; the shell waits until it ends, and it gets the console. Ctrl+C ends it |
| `bg <path>.elf.b64` | Run a program in the background: no input, output thrown away |
| `kill <id>` | End a program (ids are in `ps` and the Task Manager) |
| `open <file>` | Show a PNG, JPEG or BMP image in a window, play an MP4 video, or play a WAV sound |
| `k build <file.k>`, `k run out.kbin` | Compile and run a K program |
| `net` | Network address, gateway and DNS server |
| `ping <host>` | Send one ICMP echo |
| `dns <host or url>` | Look up a host's IPv4 address (`dns example.com`, `dns https://example.com/page`) |
| `fetch <url>` | HTTP GET; print the response |
| `browse [url]` | Open a Browser window |
| `kai <request>` | Ask K-AI, e.g. `kai what's running?` (see below) |
| `beep`, `snd <hz>`, `play <notes or file.wav>` | Sound: a beep, a steady tone (`snd 0` or `snd off` stops it), a tune or a WAV file (see [Sound](#sound)) |
| `ps` | Tasks, the core each runs on, their share of its time, which are programs, and how idle each core is |
| `heap_remain`, `heap_test` | Kernel heap usage / stress test |
| `mmu [addr]` | Free page frames; how an address translates |
| `dump` | Print the CPU registers |

The userspace programs are in `/programs` (the Programs window lists them), and typing a program's name runs it, like a command. Try `helloc` (prints, then echoes a line you type), `ui_demo` (opens a window with buttons) or `sound_demo` (a siren and a tune), or `run /programs/helloc.elf.b64` in full.

### K-AI

K-AI is KOS's assistant. Ask it things in your own words with `kai`:

```
kos:/> kai how much mem is left
K-AI: The kernel heap has 107.0 MiB free of 121.5 MiB (11% used).
kos:/> kai can you ping 10.0.2.2
K-AI: 10.0.2.2 replied in 3 ms.
kos:/> kai i dont need files/junk.txt anymore, remove it
K-AI: Delete the file /files/junk.txt? This can't be undone. (yes/no)
yes/no> y
K-AI: Deleted /files/junk.txt.
```

It can tell you the time, say what's running, how much memory is free, how long KOS has been up and whether the network is connected; ping a host; open a website in the Browser; list files; and delete a file. `kai` on its own lists this, and `kai --debug <request>` shows what it understood without doing anything.

Under the hood, a small neural network (a 1.45M-parameter transformer) reads your request and picks an *intent*, such as `ping`, and its *details*, such as the host. KOS then does it with its own functions, so the answers are live system data, never made up. When the model isn't sure, K-AI asks you to rephrase instead of guessing. Dangerous actions (deleting) need the model to be very sure, check the file exists, and only go ahead when you answer `y` or `yes`.

The model is trained on your computer, not in KOS. `tools/assistant/README.md` explains the training data, the tests and the accuracy report.

```sh
make kai-setup     # once: a Python venv with PyTorch
make kai-train     # generate data, train, export disk/kai/kai.kosm, and check KOS's copy matches PyTorch
make kai-upload    # put the model on disk.img, keeping files you wrote from inside KOS
make kai           # kai-train, then kai-upload
```

### The network

QEMU gives KOS a virtual Ethernet card and routes its traffic through your computer's connection (wifi or wired), so KOS can reach the internet. KOS configures itself with DHCP: it gets `10.0.2.15`, and your computer is reachable from inside KOS as `10.0.2.2`. To serve your own files to the Browser, run a web server on your computer, for example `python3 -m http.server 8000` in a folder, then open `http://10.0.2.2:8000/` in KOS.

### Sound

KOS has one voice, like a PC speaker (or TempleOS's `Snd`): it plays one sound at a time, and starting a new one cuts off whatever was playing. A sound is a steady square-wave tone (`snd 440`, until `snd 0`), a short `beep`, a WAV file (`play sounds/chime.wav`, or `open` it), or a tune:

```
kos:/> play T140 O4 C D E F G2 > C1
```

Tunes are written in a small music notation, like the old BASIC `PLAY`. Spaces and case don't matter.

| Notation | Meaning |
|---|---|
| `C D E F G A B` | A note. `#` or `+` after it makes it sharp, `-` flat (`F#`, `B-`) |
| a length after a note | `1` whole, `2` half, `4` quarter (the default), `8`, `16`, `32`; a `.` makes it half as long again (`C4.`) |
| `R` or `P` | A rest, with a length like a note's |
| `O0`–`O8` | The octave (4 to start, where A is 440 Hz); `<` / `>` go one down / up |
| `T32`–`T255` | Tempo, in quarter notes a minute (120 to start) |
| `L1`–`L64` | The default length for notes without one |

WAV files can be uncompressed PCM at 8, 16 or 24 bits, mono, stereo or more channels, at any rate from 8 to 192 kHz, up to three minutes long. They're mixed down to mono and converted to 48 kHz when they start. Compressed WAVs (ADPCM, float) are refused with a message saying so. `/sounds/chime.wav` on the disk is a short chime to try.

Programs get the same with `snd`, `beep`, `play` and `play_file` in C (`sys.h`) and `kernel.snd`, `kernel.beep`, `kernel.play` and `kernel.play_file` in K. For a program, `play`, `play_file` and `beep` wait until their sound is over, and anything a program is playing stops when it ends (or crashes, or is killed), so a stuck tone can't outlive it. The desktop plays a beep when a program prints a BEL (`\a`) and a low bonk when a program crashes, but only when nothing else is playing.

Underneath, `kernel/src/drivers/virtio/snd.rs` drives QEMU's virtio-sound device (16-bit, 48 kHz, in 10 ms periods), and the `sound` task (`kernel/src/sound/`) keeps it fed, generating the square wave with a 2 ms fade at each start and stop so notes don't click.

### The disk

`disk.img` is a FAT image attached as a virtio-blk device. `make disk` builds it with `tools/mkdisk` from:

- everything in `disk/` (K sources in `k/`, sounds in `sounds/`, images in `images/`, sample files);
- every program in `userspace/prog/`, as `programs/<name>.elf.b64`; and
- every system command in `userspace/cmd/`, as `bin/<name>.elf.b64`.

`make run` / `make run-gui` create it if it's missing but never overwrite it, so files you write from inside KOS persist until the next `make disk`.

## Logging and debugging

Every subsystem logs through the `log` crate, tagged with its module: `scheduler`, `syscall`, `net`, `mm::frame`, `drivers::virtio::blk`, `ui::wm`, and so on. Each line looks like:

```
[    9.579 c0 net      ] INFO  net: GET http://example.com/ -> 200 OK (577 bytes, 1076 ms)
   uptime  core task     level module: message        (task is "irq" inside an interrupt handler)
```

- **What gets recorded** is set per module: a default level plus overrides. An override covers the module and everything inside it (`net` covers `net::http`). Levels: `off`, `error`, `warn`, `info`, `debug`, `trace`.
- **Everything recorded** goes into a ring buffer of the last 1024 records, whether or not it was printed.
- **What gets printed** is a separate `console` level, `info` by default. So you can record at `trace` quietly and look later.
- **On a kernel panic or fault,** the last 40 records are printed automatically.

Set levels at boot (no rebuild needed), or live from the shell:

```sh
make run-gui LOG="net=debug,console=debug"   # watch every network request live
make run LOG=trace                            # record everything; print only info and up
```

| Shell command | What it does |
|---------------|--------------|
| `log` | Show the current levels |
| `log net trace` | Set one module's level (`log console debug` sets what's printed) |
| `log debug` / `log off` | Set the default level |
| `log net=trace,syscall=trace` | Several at once, same syntax as `LOG` |
| `log reset` | Back to the defaults |
| `dmesg [count] [text]` | Show recent records, optionally only those mentioning `text` (e.g. `dmesg 20 tcp`) |
| `stats` | Counters: interrupts by device, syscalls, context switches, disk and network I/O, frames rendered |
| `ps` | Tasks and their states |

`make test` runs the in-kernel test suite: unit tests at boot (the K compiler, the heaps), then, once the scheduler is up, tests that run tiny programs in user mode and check that they exit, get killed for touching kernel memory or overflowing their stack, can't see each other's memory, and can be killed. The kernel prints `test result: ok. N passed` (or `FAILED`) and powers off; the log is in `build/test.log`. Tests live in `kernel/src/test/` and next to the code they test.

Useful modules to trace: `scheduler` (task switches, blocking, waking), `syscall` (every program syscall with its arguments and result), `net` (DHCP, DNS, TCP, UDP, HTTP), `drivers::virtio::net` (every packet), `filesys` (every file request and how long it took), `drivers::virtio::blk` (every disk sector), `mm::allocator` and `mm::frame` (every allocation), `arch::gic` (every interrupt), `ui::wm` (windows, focus, each frame's redraw time), `loader` (ELF segments), `smoltcp` (the TCP/IP stack itself).

### With lldb

`make debug` runs KOS like `make run-gui`, with QEMU's debug server on port 1234 (`DEBUG_PORT`; add `WAIT=1` to hold every core at the first instruction, for debugging boot). In another terminal, `make lldb` attaches and stops KOS. Each core is an lldb thread (thread 1 is core 0), and the kernel has full debug info, so lldb's own commands work as usual: `bt`, `frame variable`, `breakpoint set -n syscall_uname`, `step`, `c`. Apple's lldb has no Rust language plugin (no lldb does any more), so `kos.py` loads the Rust toolchain's own formatters, the ones `rust-lldb` uses: `String`, `Vec`, `Option`, enums and structs show readably (`job = {path:"/bin/uname.elf.b64", args:"-a", console:Foreground}`). lldb still prints a one-time warning about the missing plugin, and `expr` can't parse Rust, so use `frame variable job.path` rather than `p job.path`. The kernel is optimized, so some variables show as unavailable, especially in inlined functions; going up a frame often finds them.

`tools/lldb/kos.py` (loaded by `make lldb`) adds:

| Command | What it does |
|---------|--------------|
| `kos tasks` | Every task: id, core, name, running or stopped, and where it is (function, file:line) |
| `kos bt <id>` | A task's backtrace, including a blocked one, which lldb can't see on its own (its registers are in the frame the scheduler saved) |
| `kos prog <name>` | Loads a C program's symbols from `build/user` at its load address, for breakpoints in it, source and variables (`kos prog ksh`, then `b render`) |

```
(lldb) kos bt 8
task 8 'ksh', stopped, in the kernel:
  #0  kos_kernel::scheduler::yield_now+52 at mod.rs:591
  #1  kos_kernel::scheduler::wait::sleep_ms+644
  #2  kos_kernel::syscall::dispatch_syscall+1024
  #3  rust_sync_handler+308 at mod.rs:54
```

The scheduler keeps the table these read, `KOS_DEBUG_TASKS`, in `kernel/src/scheduler/mod.rs`. Programs are built with `-g` (their copies on the disk are stripped), and everything is built with frame pointers, so every stack can be walked. Every program loads at the same address, so only one program's symbols are loaded at a time.

## Writing programs

### In C

Add a `.c` file to `userspace/prog/` and run `make disk`; it's built and put on the disk automatically.

```c
#include "../libc/sys.h"
#include "../libc/print/print.h"

void _start(void) {
    printf("Hello from %s!\n", "KOS");
    char name[64];
    long n = readln(name, sizeof(name) - 1);
    name[n] = '\0';
    printf("Hi, %s\n", name);
    exit();
}
```

Programs are freestanding: they start at `_start`, must call `exit()`, and use KOS's own C library in `userspace/libc` (on the include path, so `#include "stdio.h"` works). Each header documents its part:

| Header | What's in it |
|---|---|
| `sys.h` | The kernel's side: console I/O, memory, files (read, write, append, list folders, delete, make folders), arguments, starting and waiting for other programs, the clock, random numbers, windows with labels and buttons, and `last_error()` for why a call failed |
| `stdio.h` | `printf`, `snprintf`, `asprintf`, `puts`, with widths, precision and `%f %e %g` |
| `string.h`, `ctype.h` | The usual string, memory and character functions |
| `stdlib.h` | `malloc`/`calloc`/`realloc`, `strtol`/`strtod`/`atoi`, `qsort`/`bsearch`, `rand` |
| `math.h` | `sin cos tan asin acos atan atan2 exp log log10 pow sqrt hypot floor ceil round fmod`..., written from scratch (`math.c` explains how) |
| `time.h` | `time`, `localtime`, `gmtime`, `mktime`, `strftime` |
| `net.h` | HTTP and HTTPS requests (any method, headers and body), `ping`, TCP connections and UDP sockets |
| `crypto.h` | SHA-256, SHA-512, HMAC-SHA256 and AES-256-GCM, done by the kernel; `crypto_equal` for comparing MACs |
| `canvas.h` | Windows you draw pixels into, for graphics and games |

See `userspace/prog/ui_demo.c` for a program with a window.

#### Syscalls

Under the library, a program talks to the kernel with `svc #n`, where `n` picks the syscall. Arguments go in `x0`-`x4`, and syscalls with several jobs (4-7, 12, 13, 15, 16) take a sub-command in `x16`. The result comes back in `x0`: `-1` (`u64::MAX`) means it failed, and sub-command 5 of syscall 13 (`last_error()`) says why. Pointers are checked against the program's own memory, and a bad one fails instead of crashing anything. Strings passed with a length aren't NUL-terminated; the others are. The kernel side is `kernel/src/syscall.rs`.

| `svc` | `x16` | Arguments → result | libc |
|---|---|---|---|
| 0 | | → never returns | `exit` |
| 1 | | byte → | `write_byte` |
| 2 | | data, len → | `write_bytes`, `write`, `printf` |
| 3 | | buf, max → length (0 if not in the foreground) | `readln` |
| 4 memory | 0 | size → pointer, or 0 | `alloc`, `malloc` |
| | 1 | size, n → pointer (size × n) | `alloc_n` |
| | 2 | size → zeroed pointer | `alloc_z`, `calloc` |
| | 3 | size, n → zeroed pointer | `alloc_n_z` |
| | 4 | → free heap bytes | `mem_free` |
| | 5 | → total heap bytes | `mem_total` |
| | 6 | pointer → 0 | `free` |
| | 7 | pointer → its size, or 0 | `mem_size`, `realloc` |
| 5 files | 0 | path, len, data, len → 0 | `fs_write` |
| | 1 | path, len, buf, max → file size | `fs_read`, `fs_size`, `fs_read_all` |
| | 2 | path, len, buf, max → listing size: lines of `d`/`f`, tab, size, tab, name | `fs_list` |
| | 3 | path, len → 0 | `fs_remove` |
| | 4 | path, len → 0 | `fs_mkdir` |
| | 5 | buf, max → length of the current directory | `fs_cwd` |
| 6 windows | 0 | title, elements, count → window id | `ui_create_window` |
| | 1 | id → | `ui_close_window` |
| | 2 | → window id | `ui_test_window` |
| | 3 | id → 0 nothing, n + 1 button n clicked, -1 closed | `ui_poll_event` |
| | 4 | title, width, height → canvas id | `canvas_open` |
| | 5 | id, pixels, count → 0 | `canvas_present` |
| | 6 | id, event → 1 event, 0 none | `canvas_poll` |
| | 7 | id → | `canvas_close` |
| 7 time | 0 | ms → (sleeps) | `sleep_ms` |
| | 1 | seconds → (sleeps) | `sleep_s` |
| | 2 | → ms since boot | `uptime_ms` |
| | 3 | → seconds since 1970 (UTC) | `unix_time`, `time` |
| | 4 | → time zone's offset from UTC, in seconds | `utc_offset`, `localtime` |
| 8 | | → (prints the registers) | `dump` |
| 9 | | → (gives up the time slice) | `yield` |
| 10 | | buf, max → length of the arguments | `args`, `get_argv` |
| 11 | | buf, len → 0 (fills it with random bytes from the kernel's ChaCha20 generator) | `random_bytes`, `random_u64` |
| 12 network | 0 | request (`http_request_t*`) → status code; the kernel keeps the response | `http_request`, `http_get`, `http_post` |
| | 1 | buf, max → the kept response's body length | (inside `http_request`) |
| | 2 | buf, max → its headers' length (`Name: value` lines) | (inside `http_request`) |
| | 3 | host → round trip in ms | `ping` |
| | 4 | buf, max → length of this machine's address | `net_address` |
| | 5 | host, port → connection id | `tcp_connect` |
| | 6 | id, data, len → bytes sent | `tcp_send`, `tcp_send_str` |
| | 7 | id, buf, max, don't wait → bytes read; 0 closed; -2 nothing yet | `tcp_recv`, `tcp_try_recv`, `tcp_recv_line` |
| | 8 | id → | `tcp_close` |
| | 9 | port (0 = any) → socket id | `udp_open` |
| | 10 | id, to (`{host, port}`), data, len → 0 | `udp_send`, `udp_send_str` |
| | 11 | id, buf, max, from (`{IPv4 address, port}`, may be null), don't wait → datagram length; -2 nothing yet | `udp_recv`, `udp_try_recv` |
| | 12 | id → | `udp_close` |
| 13 processes | 0 | path, args → task id | `spawn` |
| | 1 | id → 0 once it has ended | `proc_wait`, `run_program` |
| | 2 | id → 0 | `proc_kill` |
| | 3 | → this program's id | `proc_self` |
| | 4 | id → 1 running, 0 not | `proc_running` |
| | 5 | buf, max → length of why the last call failed | `last_error` |
| 14 | | buf → 0 (fills a `utsname_t`) | `uname` |
| 15 crypto | 0 | data, len, out → 0 (32-byte SHA-256) | `sha256` |
| | 1 | data, len, out → 0 (64-byte SHA-512) | `sha512` |
| | 2 | key, len, data, len, out → 0 (32-byte HMAC-SHA256) | `hmac_sha256` |
| | 3 | `aead_t*` → 0 (encrypts in place, writes the tag) | `aes_gcm_seal` |
| | 4 | `aead_t*` → 0, or -1 if the tag doesn't match (decrypts in place) | `aes_gcm_open` |
| 16 sound | 0 | hz → 0 (a steady tone until the next sound; 0 is silence) | `snd` |
| | 1 | → 0, once the beep is over | `beep` |
| | 2 | notes, len → 0 once the tune is over, or -1 for bad notes | `play` |
| | 3 | → 0 (stops whatever is playing) | `snd_stop` |
| | 4 | → 1 if something is playing, else 0 | `snd_busy` |
| | 5 | path, len → 0 once the WAV file is over, or -1 (`last_error` says why) | `play_file` |

Calls that copy something out (a file, a listing, a response) return its full length, even when the buffer was smaller, so passing a buffer of 0 bytes asks how big it is. The libc functions do that for you: ask the size, allocate, then copy.

For graphics and games, `userspace/libc/canvas.h` gives a program a window it draws pixels into: lines, rectangles, circles and text, with mouse and keyboard events, presented a whole frame at a time. `canvas_demo` is a sketch pad with a bouncing ball, and `calculator` a scientific calculator (click or type an expression with `sin`, `cos`, `exp`, `ln`, `log`, `^`, `pi` and `e`; Enter for the result); their sources in `userspace/prog/` show how it fits together.

Assembly (`.s`) files in `userspace/prog/` work the same way.

### Outside this repo

`tools/kos-cc` builds programs from anywhere on your computer: `cargo run --release -p kos-cc -- game.c` writes `game.elf`, linked with `userspace/libc` (which is on the include path, so `#include "stdio.h"`, `"net.h"` and so on). `-o` names the output, `--b64` also writes `<out>.b64`, and anything after `--` goes to clang. Put the `.elf` in `disk/`, `make disk`, and `run game.elf` (or the `.elf.b64` in `disk/programs/`, for the Programs window).

### System commands

A `.c` file in `userspace/cmd/` is a system command instead: `make disk` puts it in `/bin` (not the Programs window), and typing its name in the shell runs it, with whatever follows as its arguments. `args()` / `get_argv()` in `sys.h` read them. `userspace/cmd/grep.c` is the first one: `grep [-i] [-n] <text> <file>...`. `run` and `bg` pass arguments too: `run /programs/prog.elf.b64 a b`. Programs in `/programs` run by name too, but a command in `/bin` wins if both have the name.

`ssh [-p port] user@host [command]` logs in to another computer with a password (OpenSSH servers work out of the box: curve25519-sha256, ssh-ed25519 host keys, aes256-gcm@openssh.com), or runs one command there. The Terminal is line-based, so it is too: each line typed goes to the remote shell and the output comes back with escape codes stripped, so full-screen programs like vim don't work; `~.` on its own disconnects. Host keys are remembered in `/.kos/ssh/known_hosts`, and a changed one is refused. The crypto it needs beyond `crypto.h` (X25519, Ed25519) is in `ssh.c` itself.

`ksh` (`userspace/cmd/ksh.c`) is a second shell, a C program in its own Shell window: type `ksh` in the Terminal. It has its own builtins (`ls`, `cd`, `cat`, `echo`, `alias`, `jobs`, ...), chains commands with `;`, `&&`, `||` and `&`, and has variables (`$X`, `$?`), quotes, `*` globs, history, Tab completion and scripts (`source file`; `/.kshrc` runs at the start). Programs it runs print in the Terminal, and it has no pipes or redirection yet. `help` in it lists everything.

`audio <file>` (`userspace/cmd/audio.c`) is an audio analyzer in its own window, with three tabs (1, 2, 3 or Tab): the waveform (each channel's peaks, with the RMS inside), a spectrogram in magma colors (0 to -100 dB) and the frequency spectrum (Welch-averaged, with the loudest peak and its note). It reads WAV (8/16/24/32-bit PCM, float, mu-law, A-law, IMA ADPCM), AIFF / AIFF-C and Sun `.au`; compressed formats (MP3, FLAC, Ogg) aren't supported yet. The mouse wheel zooms, right-drag scrolls, left-drag selects a stretch (the spectrum then shows only it, and `z` zooms to it), `0` shows everything, `[` / `]` change the FFT size and `l` switches the frequency axis between linear and log. Hovering shows the time, value, frequency, note and dB under the pointer. Space plays the selection (or from the left of the view) with a moving playhead: it writes a 16-bit mono copy to `/.kos/audio/play.wav` for `play_file`, so it's limited to three minutes and 8-192 kHz.

`files` (`userspace/cmd/files.c`) is a file explorer in its own Files window: `files [folder]` in the Terminal. Click a file to preview it (text, or hex for binaries); double-click or Enter opens a folder, runs a program or opens a text file in `edit`. Backspace (or the Up button) goes up, Delete deletes after a `y`, `n` makes a folder, `r` reloads and a letter jumps to the next name starting with it.

`edit` (`userspace/cmd/edit.c`) is a text editor in its own window: `edit [file]` (a new file is made when you save). Click to place the cursor, drag or double-click to select, scroll with the wheel. Ctrl+S saves, Ctrl+F finds (Enter again for the next match), Ctrl+G goes to a line, Ctrl+Z / Ctrl+Y undo and redo, Ctrl+A selects all, Ctrl+X / Ctrl+C / Ctrl+V cut, copy and paste (the whole line when nothing is selected), and Ctrl+Q quits, asking first about unsaved changes. Programs' canvas windows get Ctrl+letter as 1-26 (`CANVAS_CTRL('s')`) and the mouse wheel as `CANVAS_SCROLL`.

### In K

K is a small statically typed language compiled to AArch64 by a compiler built into the kernel (`kernel/src/k/`).

It reads a bit like a stripped-down Rust: a file starts with `module name;`, functions are `fn add(a: u64, b: u64) -> u64 { ... }`, and variables are `let` (immutable) or `mut`, always with a type. The types are the fixed-size integers (`u8`–`u64`, `i8`–`i64`, `usize`, `isize`), `bool`, `char`, string literals, fixed arrays (`u64[4]`) and pointers (`u64 pointer`, with `&x` and `*p`), plus your own `struct`s and `enum`s (matched with `match`). Control flow is `if`/`else`, `while`, `loop`, `break` and `continue`. There's no standard library: after `import kernel;` a program talks to the OS through a handful of built-ins, `kernel.print("...")`, `kernel.write_byte`, `kernel.write_bytes(ptr, len)`, `kernel.read_line`, `kernel.sleep_ms(ms)`, `kernel.dump()` and `kernel.exit(code)`, plus sound with `kernel.snd(hz)`, `kernel.beep()`, `kernel.play("notes")` and `kernel.play_file("file.wav")`, which compile to syscalls. The compiler goes lexer → parser → type checker → IR → AArch64 machine code and packs the result into a KBIN file, which `k run` loads into user memory and runs as an ordinary process, just like a C program.

There are examples in `/k` on the disk (`basic.k`, `h.k`, and `song.k`, which plays a scale and a tune). `k build` writes `out.kbin` to the current folder:

```
kos:/> cd k
kos:/k> cat h.k
kos:/k> k build h.k
kos:/k> k run out.kbin
```

## How it works

- **Boot:** QEMU runs `bootloader.bin` from flash. It copies the kernel (embedded in it) to `0x4020_0000` and starts it. A small assembly stub (`kernel/src/arch/boot.s`) turns on the MMU and jumps into the kernel at its high address, then the boot steps registered with `#[initcall]` run in order and core 0 starts the scheduler. The other cores are started via PSCI.
- **Memory:** the kernel runs in the higher half (`0xFFFF_FF80_0000_0000` + physical) through TTBR1, with separate permissions for code, read-only data and writable data. The low half (TTBR0) is the running program's own address space (see Programs below). The kernel heap (a free-list allocator that merges freed blocks) and the page frames share all the RAM above the kernel, with no fixed split: the heap starts at 32 MiB and grows upward into free memory when it runs out (and shrinks back when the end of it is free again), while page frames, for page tables and programs, are handed out from the top of RAM down. So the Browser can use most of the 512 MiB, or programs can.
- **Tasks:** each core that runs tasks has its own preemptive round-robin scheduler, with 10 ms time slices: core 0 runs the kernel's tasks (shell, filesystem, network, UI), and cores 3 and 4 run programs, each new one going to whichever has fewer. (Core 1 runs `std::thread` work and core 2 async work, outside any scheduler.) Tasks block on wait queues until an interrupt or another task wakes them, and waking a task on another core sends that core an interrupt (a GIC SGI) so it switches right away. A core sleeps (`wfi`) when nothing is ready. Every switch saves the full register state, including floating-point/SIMD registers.
- **Drivers:** interrupts come through a GICv2. The PL011 UART, virtio-blk and virtio-net raise interrupts; virtio-input provides the keyboard and tablet; the screen is a framebuffer shown through virtio-gpu (2D), or QEMU's ramfb if there's no GPU (`make run-gui DISPLAY_DEV=ramfb`); the PL031 real-time clock gives the date and time (in UTC, with the host's time zone passed in at boot).
- **Network:** a network task owns the virtio-net device and the smoltcp TCP/IP stack. It configures itself over DHCP and handles DNS lookups, HTTP requests and pings for the rest of the kernel, and TCP connections and UDP sockets for programs, each as a small state machine so they don't block each other. HTTPS runs TLS 1.3 (`embedded-tls`) in a separate `tls` task, keyed by the kernel's random number generator (ChaCha20, seeded and reseeded from the CPU's hardware one, `RNDR`), with the network task carrying its encrypted bytes.
- **Browser:** pages are rendered by [lighthouse](kernel/crates/lighthouse), a small HTML/CSS engine ported to `no_std`: parse, style, lay out (with word wrapping), then paint into a display list. The Browser keeps only that list and draws just the part in view. It draws images and form fields itself, as boxes lighthouse leaves room for.
- **K-AI:** the `kai` crate (`kernel/crates/kai`) runs the model, a tokenizer and transformer written to match the PyTorch original exactly. `kernel/src/assistant` loads it from the disk and turns what it understood into actions.
- **Desktop:** a compositor in the UI task draws windows into a back buffer and copies only the changed regions to the screen. Each window's contents come from an `App`. Programs talk to it through syscalls.
- **Programs** run in user mode (EL0), each in its own address space (`kernel/src/process.rs`). Its page table is built from page frames as it needs them: its image at `0x5000_0000`, a heap after it whose pages are mapped as it allocates and given back as it frees, and a stack far above that starts at 16 KiB and grows (up to 8 MiB) when the program touches the page below it, with unmapped space in between. So how big a program gets is only limited by free memory, and it only holds what it uses. It can't see the kernel or any other program, and the table's ASID keeps its TLB entries apart, so switching programs needs no TLB flush. Syscalls check every pointer they're handed against the program's memory. A program that touches anything else, or runs a privileged instruction, is killed, along with its windows, and the kernel carries on. Up to 255 run at once (one ASID each); `kill` (or End task) marks a program, and it's removed the next time it would run in user mode, never halfway through a syscall.

## Source layout

```
bootloader/            first-stage loader: copies the kernel to 0x4020_0000, parks secondary cores
kernel/
  linker.ld            higher-half layout + linker sections for initcalls and syscalls
  crates/kos-derive/   #[initcall], #[syscall], const_assert! proc macros
  crates/lighthouse/   HTML/CSS rendering engine (no_std port), used by the Browser
  crates/kai/          K-AI's tokenizer and model inference (no_std)
  src/
    main.rs            rust_main: run initcalls, start the scheduler
    init/              initcall runner and the boot initcalls (calls.rs)
    arch/              AArch64: boot stub, exception vectors, MMU, GIC, timer, PSCI, secondary cores
    mm/                page frames, address spaces (vm.rs), kernel heap, memory map (layout.rs)
    scheduler/         tasks, trap frame, wait queues, async executor
    syscall.rs         syscall handlers (svc #N)
    loader/            ELF loading and the program runner task
    drivers/           UART, display (virtio-gpu or ramfb + fw_cfg), virtio (blk, gpu, input, net, snd), input events, panic screen
    net/               network task: DHCP, DNS, HTTP client, TLS, ping, TCP + UDP sockets (on smoltcp)
    sound/             the one-voice sound system: tones, tunes and their notation, WAV decoding (wav.rs), the task feeding the sound device
    rand/              random numbers (ChaCha20, seeded by RNDR)
    crypto.rs          hashes, HMAC, AES-GCM for the crypto syscall
    assistant/         K-AI: loading the model, its tools, confirmations
    filesys/           fs_task server, FAT and SimpleFS backends
    shell/             the shell: line input, commands
    ui/                compositor + window manager (wm.rs), apps/, drawing (gfx.rs), console mirror
    k/                 the K compiler (lexer → parser → typecheck → IR → AArch64 → KBIN)
    std/               in-kernel std: locks, Rc/RefCell, hash map, channels, threads, io::{Read, Write}, net::{TcpStream, UdpSocket}
userspace/
  libc/                the C library: syscalls (sys.h), printf (print/print.h), canvases (canvas.h), maths (math.h)
  prog/                programs; every .c / .s file here is built and put on the disk
disk/                  files that go on disk.img
tools/mkdisk/          host tool that builds disk.img (and puts files into an existing one)
tools/assistant/       K-AI's training: intents, data generator, model, tests, export
```
