# KOS build & run.
#
#   make            build build/kos.bin (kernel embedded in the bootloader)
#   make run        boot with serial console in this terminal, no display
#   make run-gui    boot with a display window + serial in this terminal
#                   (DISPLAY_DEV=ramfb for the plain framebuffer instead of virtio-gpu)
#   make debug      run-gui, with QEMU's debug server on DEBUG_PORT (1234) for
#                   lldb; WAIT=1 holds every core at the first instruction
#   make lldb       attach lldb to `make debug` (in another terminal), with the
#                   kernel's symbols and tools/lldb/kos.py (`kos help`)
#   make test       build with the in-kernel test suite, boot it, and exit 0
#                   if every test passed (the log is in build/test.log)
#   make user       build the userspace example programs into build/user/
#   make games      build every game in games/ into build/user/games/
#   make disk       (re)build disk.img from disk/ + the userspace programs
#                   (into /programs), the system commands (into /bin) and
#                   the games (into /games), discarding anything written to
#                   it from inside KOS
#   make kai-setup  create K-AI's Python venv with PyTorch (once)
#   make kai-train  regenerate K-AI's data, train and export the model, and
#                   check KOS's Rust copy gives the same results as PyTorch
#   make kai-upload put the exported model on disk.img, keeping its other files
#   make kai        kai-train, then kai-upload
#   make digit-setup create the digit recogniser's Python venv (once)
#   make digit-train make its training data, train it and export the model
#   make digit-upload put the exported recogniser on disk.img
#   make doc        build the kernel's API docs (rustdoc, private items too) into
#                   target/aarch64-unknown-none/doc/; OPEN=1 opens them in a browser
#   make clean
#
# Kernel cargo features go in FEATURES (e.g. FEATURES=test); log levels go in
# LOG, e.g. `make run-gui LOG="net=trace,console=debug"`. ACCEL=hvf runs KOS
# on the Mac's CPU instead of emulating it: `make run-gui ACCEL=hvf`.

BUILD    := build
comma    := ,
TARGET   := target/aarch64-unknown-none/release
FEATURES ?=
# The clock chip runs in UTC; this is how far this computer's time zone is
# from it ("+0100"), so KOS can show local time.
UTC_OFFSET ?= $(shell date +%z)
# Log filters, e.g. LOG="debug" or LOG="net=trace,console=debug" (see klog.rs).
LOG      ?=
DISK     ?= disk.img
# Core 0 boots; cores 1-2 run the thread queue, cores 3-4 the async executor.
SMP      ?= 5
MEM      ?= 512M
# QEMU display backend for run-gui: cocoa on macOS, gtk or sdl on Linux.
# zoom-to-fit scales the 1024x768 screen to the window: resize it, or go
# full screen, and KOS grows with it (the mouse still lines up).
GUI      ?= $(if $(filter Darwin,$(shell uname -s)),cocoa,gtk)$(comma)zoom-to-fit=on
# The display device for run-gui: virtio-gpu or ramfb. KOS uses whichever
# it finds.
DISPLAY_DEV ?= virtio-gpu
DISPLAY_DEVICE := $(if $(filter ramfb,$(DISPLAY_DEV)),ramfb,virtio-gpu-device)
# How QEMU runs KOS's code: tcg emulates every instruction (works anywhere,
# slowly); hvf runs it directly on an Apple silicon Mac's CPU, through
# macOS's Hypervisor framework, many times faster.
ACCEL    ?= tcg
CPU      := $(if $(filter hvf,$(ACCEL)),host,max)
# Where KOS's sound goes: coreaudio on macOS, pa (PulseAudio) on Linux;
# AUDIO=none for silence, or AUDIO="wav,path=out.wav" to record it.
AUDIO    ?= $(if $(filter Darwin,$(shell uname -s)),coreaudio,pa)

# Ports on this computer passed through to KOS, as QEMU hostfwd rules
# separated by spaces: FWD="tcp::8080-:80" lets this computer's browser
# reach a server on KOS's port 80 at localhost:8080.
FWD      ?=
# make test listens on TEST_PORT inside KOS and reaches it from KOS
# itself through this computer's TEST_HOST_PORT (see test/net.rs).
TEST_PORT      := 7321
TEST_HOST_PORT := 47321

KERNEL_BIN := $(BUILD)/kernel.bin
KOS_BIN    := $(BUILD)/kos.bin

# Recursive (=), so a target can add to FWD.
QEMU = qemu-system-aarch64 \
	-machine virt,secure=off,highmem=off -accel $(ACCEL) -cpu $(CPU) -smp $(SMP) -m $(MEM) \
	-bios $(KOS_BIN) \
	-global virtio-mmio.force-legacy=false \
	-drive file=$(DISK),if=none,format=raw,id=hd0 -device virtio-blk-device,drive=hd0 \
	-device virtio-keyboard-device -device virtio-tablet-device \
	-netdev user,id=net0$(foreach f,$(FWD),$(comma)hostfwd=$(f)) -device virtio-net-device,netdev=net0 \
	-audiodev $(AUDIO),id=snd0 -device virtio-sound-device,audiodev=snd0,streams=1 \
	-rtc base=utc -fw_cfg name=opt/kos/utc_offset,string=$(UTC_OFFSET) \
	$(if $(LOG),-fw_cfg name=opt/kos/log$(comma)string=$(subst $(comma),$(comma)$(comma),$(LOG)))

.PHONY: all kernel kos run run-gui debug lldb test doc user games disk kai kai-setup kai-train kai-upload digit-setup digit-train digit-upload clean FORCE

all: kos

kernel: $(KERNEL_BIN)
kos: $(KOS_BIN)

# Cargo tracks its own dependencies, so always ask it; it's a no-op when
# nothing changed.
$(KERNEL_BIN): FORCE | $(BUILD)
	cd kernel && cargo build --release --features "$(FEATURES)"
	rust-objcopy $(TARGET)/kos-kernel -O binary $@

$(KOS_BIN): $(KERNEL_BIN) FORCE
	cd bootloader && cargo build --release
	rust-objcopy $(TARGET)/kos-bootloader -O binary $@

run: kos | $(DISK)
	$(QEMU) -nographic

run-gui: kos | $(DISK)
	$(QEMU) -device $(DISPLAY_DEVICE) -serial mon:stdio -display $(GUI)

DEBUG_PORT ?= 1234

debug: kos | $(DISK)
	$(QEMU) -device $(DISPLAY_DEVICE) -serial mon:stdio -display $(GUI) -gdb tcp::$(DEBUG_PORT) $(if $(WAIT),-S)

lldb:
	lldb $(TARGET)/kos-kernel -o "command script import tools/lldb/kos.py" -o "kos attach $(DEBUG_PORT)"

# The kernel powers off after the last test, printing "test result: ok"
# or "test result: FAILED"; anything else (a hang you quit) also fails.
# Sound goes to build/test.wav instead of the speakers.
test: FWD += tcp:127.0.0.1:$(TEST_HOST_PORT)-:$(TEST_PORT)
test: | $(DISK)
	$(MAKE) kos FEATURES="$(FEATURES) test"
	@rm -f $(BUILD)/test.wav
	$(subst -audiodev $(AUDIO)$(comma),-audiodev wav$(comma)path=$(BUILD)/test.wav$(comma),$(QEMU)) -nographic | tee $(BUILD)/test.log
	@if grep -q "test result: ok" $(BUILD)/test.log; then \
		echo "make test: all tests passed"; \
	else \
		echo "make test: FAILED (see $(BUILD)/test.log)"; exit 1; \
	fi

# The kernel is a binary crate, so its docs include private items; only
# its own crates are documented, not the dependencies.
doc:
	cd kernel && cargo doc --no-deps --document-private-items $(if $(OPEN),--open)
	@echo "make doc: open target/aarch64-unknown-none/doc/kos_kernel/index.html"

# ---- userspace ----------------------------------------------------------
# Programs are loaded by `run <file>` in the shell, which expects the ELF
# base64-encoded on disk, so each program also gets a <name>.elf.b64 in
# build/user/disk/programs/, which is what goes on the disk (in /programs,
# where the Programs window lists them).

USER_CC     := clang -target aarch64-none-elf -fuse-ld=lld -nostdlib \
	-fPIE -static -Wl,-pie -Wl,--image-base=0
# -g for lldb (`kos prog`): build/user keeps the debug info, the copies on
# the disk are stripped of it.
USER_CFLAGS := -ffreestanding -Wall -g -I userspace/libc
# The C library: every .c and .h in userspace/libc and its subfolders.
LIBC_SRC    := $(wildcard userspace/libc/*.c userspace/libc/*/*.c)
LIBC_H      := $(wildcard userspace/libc/*.h userspace/libc/*/*.h)
USER_OUT    := $(BUILD)/user
# Every .c / .s file in userspace/prog is a program. C programs link the libc.
USER_SRC    := $(wildcard userspace/prog/*.c userspace/prog/*.s)
USER_PROGS  := $(patsubst userspace/prog/%,$(USER_OUT)/%.elf,$(basename $(USER_SRC)))
USER_B64    := $(patsubst $(USER_OUT)/%,$(USER_OUT)/disk/programs/%.b64,$(USER_PROGS))
# Every .c file in userspace/cmd is a system command (grep, ...): it goes in
# /bin on the disk instead of /programs, so it isn't in the Programs window,
# and the shell runs it when you type its name.
CMD_SRC     := $(wildcard userspace/cmd/*.c)
CMD_OUT     := $(USER_OUT)/cmd
CMD_PROGS   := $(patsubst userspace/cmd/%.c,$(CMD_OUT)/%.elf,$(CMD_SRC))
# Laid out as the disk will be: build/user/cmd/disk/bin/<name>.elf.b64.
CMD_B64     := $(patsubst $(CMD_OUT)/%,$(CMD_OUT)/disk/bin/%.b64,$(CMD_PROGS))

user: $(USER_PROGS) $(USER_B64) $(CMD_PROGS) $(CMD_B64)

$(USER_OUT)/%.elf: userspace/prog/%.c $(LIBC_SRC) $(LIBC_H) | $(USER_OUT)
	$(USER_CC) $(USER_CFLAGS) -o $@ $(filter %.c,$^)

$(USER_OUT)/%.elf: userspace/prog/%.s | $(USER_OUT)
	$(USER_CC) -o $@ $<

$(USER_OUT)/disk/programs/%.elf.b64: $(USER_OUT)/%.elf | $(USER_OUT)/disk/programs
	rust-objcopy --strip-debug $< $@.tmp
	base64 < $@.tmp | tr -d '\n' > $@
	rm $@.tmp

$(CMD_OUT)/%.elf: userspace/cmd/%.c $(LIBC_SRC) $(LIBC_H) | $(CMD_OUT)
	$(USER_CC) $(USER_CFLAGS) -o $@ $(filter %.c,$^)

$(CMD_OUT)/disk/bin/%.elf.b64: $(CMD_OUT)/%.elf | $(CMD_OUT)/disk/bin
	rust-objcopy --strip-debug $< $@.tmp
	base64 < $@.tmp | tr -d '\n' > $@
	rm $@.tmp

# ---- games --------------------------------------------------------------
# Every folder in games/ is a game: its .c and .s files are built into one
# program named after the folder, which goes on the disk as a plain ELF,
# games/<name>/ -> /games/<name>.elf (`run /games/<name>.elf` in the shell).
# build/user/games/debug keeps the debug info; the disk's copies are stripped.

GAME_DIRS := $(patsubst %/,%,$(wildcard games/*/))
GAME_OUT  := $(USER_OUT)/games
GAME_DEBUG := $(patsubst games/%,$(GAME_OUT)/debug/%.elf,$(GAME_DIRS))
GAME_ELFS  := $(patsubst games/%,$(GAME_OUT)/disk/games/%.elf,$(GAME_DIRS))

games: $(GAME_DEBUG) $(GAME_ELFS)

.SECONDEXPANSION:
$(GAME_OUT)/debug/%.elf: $$(wildcard games/$$*/*.c games/$$*/*.s) $(LIBC_SRC) $(LIBC_H) | $(GAME_OUT)/debug
	$(USER_CC) $(USER_CFLAGS) -o $@ $(filter %.c %.s,$^)

$(GAME_OUT)/disk/games/%.elf: $(GAME_OUT)/debug/%.elf | $(GAME_OUT)/disk/games
	rust-objcopy --strip-debug $< $@

# ---- disk ---------------------------------------------------------------
# disk.img is only created when missing, so `make run` never wipes what you
# wrote to it. `make disk` rebuilds it from scratch.

DISK_MIB := 32

$(DISK):
	$(MAKE) disk

disk: user games
	@# Leave out programs whose sources were deleted.
	rm -f $(filter-out $(USER_B64) $(CMD_B64) $(GAME_ELFS),$(wildcard $(USER_OUT)/disk/programs/* $(CMD_OUT)/disk/bin/* $(GAME_OUT)/disk/games/*))
	cargo run -q --release -p kos-mkdisk -- $(DISK) $(DISK_MIB) disk $(USER_OUT)/disk $(CMD_OUT)/disk $(GAME_OUT)/disk

# ---- K-AI ---------------------------------------------------------------
# Training runs on this computer (see tools/assistant/README.md); the model
# goes to disk/kai/kai.kosm, and from there onto KOS's disk.

KAI_DIR := tools/assistant
KAI_PY  := .venv/bin/python

kai-setup:
	cd $(KAI_DIR) && python3 -m venv .venv && .venv/bin/pip install -q torch numpy

kai-train:
	@test -x $(KAI_DIR)/$(KAI_PY) || { echo "No venv yet: run 'make kai-setup' first."; exit 1; }
	cd $(KAI_DIR) && $(KAI_PY) check_tests.py >/dev/null && $(KAI_PY) gen_data.py && $(KAI_PY) train.py && $(KAI_PY) export.py
	@# The Rust copy KOS runs must give the same answers as PyTorch.
	cd kernel/crates/kai && cargo test -q --release

kai-upload: | $(DISK)
	cargo run -q --release -p kos-mkdisk -- --put $(DISK) disk/kai kai

kai: kai-train kai-upload

# Paint's digit recogniser, trained like K-AI (see tools/digitclass/README.md).
DIGIT_DIR := tools/digitclass

digit-setup:
	cd $(DIGIT_DIR) && python3 -m venv .venv && .venv/bin/pip install -q torch numpy pillow pandas pyarrow huggingface_hub scikit-image

digit-train:
	@test -x $(DIGIT_DIR)/$(KAI_PY) || { echo "No venv yet: run 'make digit-setup' first."; exit 1; }
	cd $(DIGIT_DIR) && $(KAI_PY) data.py && $(KAI_PY) train.py && $(KAI_PY) export.py
	@# The Rust copy KOS runs must give the same answers as PyTorch.
	cd kernel/crates/kai && cargo test -q --release --test digits

digit-upload: | $(DISK)
	cargo run -q --release -p kos-mkdisk -- --put $(DISK) disk/digits digits

# ---- misc ---------------------------------------------------------------

$(BUILD) $(USER_OUT) $(USER_OUT)/disk/programs $(CMD_OUT) $(CMD_OUT)/disk/bin $(GAME_OUT)/debug $(GAME_OUT)/disk/games:
	mkdir -p $@

clean:
	cargo clean
	rm -rf $(BUILD)
