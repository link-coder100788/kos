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
#   make tcc-libs   build the libraries programs compiled by tcc link with
#                   (build/tcc-libs/libkos.a and libtcc1.a)
#   make tcc-progs  build userspace/tcc/*.c with tcc into build/tcc-libs/prog/
#   make tcc-kos    build tcc itself for KOS: build/tcc-libs/bin/tcc.elf
#   make tcc-disk   lay out tcc's files for the disk (/bin/tcc, /include,
#                   /lib/tcc) in build/tcc-libs/disk; `make disk` does this
#   make disk       (re)build disk.img from disk/ + the userspace programs
#                   (into /programs), the system commands (into /bin), the
#                   games (into /games) and tcc, discarding anything written to
#                   it from inside KOS
#   make kai-setup  create K-AI's Python venv with PyTorch (once)
#   make kai-train  regenerate K-AI's data, train and export the model, and
#                   check KOS's Rust copy gives the same results as PyTorch
#   make kai-upload put the exported model on disk.img, keeping its other files
#   make kai        kai-train, then kai-upload
#   make digit-setup create the digit recogniser's Python venv (once)
#   make digit-train make its training data, train it and export the model
#   make digit-upload put the exported recogniser on disk.img
#   make package    build build/KOS.app: KOS, a fresh disk and QEMU (with
#                   its libraries) in one double-clickable Mac app
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

.PHONY: all kernel kos run run-gui debug lldb test doc package user games tcc-libs tcc-progs tcc-kos tcc-disk disk kai kai-setup kai-train kai-upload digit-setup digit-train digit-upload clean FORCE

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

# KOS.app: build/kos.bin, a fresh disk (not $(DISK), which may be in use)
# and QEMU_BIN with every library it needs, by tools/package_app.py. The
# app's launcher repeats the QEMU flags above: keep the two in step.
QEMU_BIN    ?= $(shell command -v qemu-system-aarch64)
PACKAGE_DIR := $(BUILD)/package

package:
	$(MAKE) kos FEATURES=
	@mkdir -p $(PACKAGE_DIR)
	rm -f $(PACKAGE_DIR)/disk.img
	$(MAKE) disk DISK=$(PACKAGE_DIR)/disk.img
	python3 tools/package_app.py --qemu "$(QEMU_BIN)" --kos $(KOS_BIN) --disk $(PACKAGE_DIR)/disk.img --out $(BUILD)/KOS.app
	@echo "make package: open $(BUILD)/KOS.app, or copy it to /Applications"

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

# ---- tcc ----------------------------------------------------------------
# TinyCC (vendored in tools/tinycc) compiles programs on KOS itself. They
# link with userspace/tcclibc, a copy of the C library that tcc can build:
# its syscall wrappers all call kos_syscall (syscall.S) instead of using
# inline asm, which tcc can't compile, so tcc builds it into libc.a and
# nothing it links was made by clang. libtcc1.a is tcc's own helpers
# (128-bit long double, atomics...). Both archives are made by tcc's
# archiver, `tcc -ar`, whose format tcc reads (macOS's ar writes another).
# crt1.o, crti.o and crtn.o (tcclibc/crt) are the start files tcc links
# into every program it builds on KOS.

TCC_DIR   := tools/tinycc
# The cross compiler: runs on this computer, makes arm64 ELF code.
TCC_X     := $(TCC_DIR)/arm64-tcc
TCC_OUT   := $(BUILD)/tcc-libs
# KOS's headers, then tcc's (stdarg.h, stddef.h...), none of this computer's.
TCC_INC   := -nostdinc -I userspace/tcclibc -I $(TCC_DIR)/include
TCC_LIBC_SRC := $(filter-out userspace/tcclibc/crt/%,$(wildcard userspace/tcclibc/*.c userspace/tcclibc/*/*.c userspace/tcclibc/*.S))
TCC_LIBC_H   := $(wildcard userspace/tcclibc/*.h userspace/tcclibc/*/*.h)
TCC_LIBC_OBJS := $(patsubst userspace/tcclibc/%,$(TCC_OUT)/obj/%.o,$(basename $(TCC_LIBC_SRC)))
TCC_CRT    := $(TCC_OUT)/crt1.o $(TCC_OUT)/crti.o $(TCC_OUT)/crtn.o
TCC_LIBS   := $(TCC_OUT)/libc.a $(TCC_OUT)/libtcc1.a

tcc-libs: $(TCC_LIBS) $(TCC_CRT)

# tcc's own Makefile knows when the compiler or its libtcc1 is out of date;
# `cross-arm64` brings both up to date.
$(TCC_X): FORCE
	$(MAKE) -C $(TCC_DIR) cross-arm64
$(TCC_DIR)/arm64-libtcc1.a: $(TCC_X) ;

$(TCC_OUT)/obj/%.o: userspace/tcclibc/%.c $(TCC_LIBC_H) $(TCC_X)
	@mkdir -p $(@D)
	$(TCC_X) $(TCC_INC) -Wall -c $< -o $@

$(TCC_OUT)/obj/%.o: userspace/tcclibc/%.S $(TCC_X)
	@mkdir -p $(@D)
	$(TCC_X) -c $< -o $@

$(TCC_OUT)/crt%.o: userspace/tcclibc/crt/crt%.c $(TCC_LIBC_H) $(TCC_X)
	@mkdir -p $(@D)
	$(TCC_X) $(TCC_INC) -Wall -c $< -o $@

$(TCC_OUT)/libc.a: $(TCC_LIBC_OBJS) $(TCC_X)
	rm -f $@
	$(TCC_X) -ar rcs $@ $(TCC_LIBC_OBJS)

$(TCC_OUT)/libtcc1.a: $(TCC_DIR)/arm64-libtcc1.a
	@mkdir -p $(@D)
	cp $< $@

# Every .c file in userspace/tcc is a program built by the cross compiler,
# the way /bin/tcc will build them on KOS: no start files, and linked to run
# at USER_BASE (0x5000_0000) rather than position-independent. Run one with
# `run <name>.elf`.
TCC_CFLAGS := $(TCC_INC) -nostdlib -static -Wl,-Ttext=0x50000000
TCC_PROGS  := $(patsubst userspace/tcc/%.c,$(TCC_OUT)/prog/%.elf,$(wildcard userspace/tcc/*.c))

tcc-progs: $(TCC_PROGS)

$(TCC_OUT)/prog/%.elf: userspace/tcc/%.c $(TCC_LIBC_H) $(TCC_LIBS)
	@mkdir -p $(@D)
	$(TCC_X) $(TCC_CFLAGS) -o $@ $< $(TCC_LIBS)

# tcc itself, for KOS: built by the cross compiler against tcclibc, with
# CONFIG_TCC_KOS for the "KOS patch" changes in tools/tinycc (no -run,
# programs linked at USER_BASE and always static). On KOS it finds its own
# headers and libraries in /lib/tcc ({B}) and tcclibc's headers in /include.
TCC_KOS_DEFS := -DONE_SOURCE=1 -DTCC_TARGET_ARM64=1 -DCONFIG_TCC_KOS -DCONFIG_TCC_STATIC \
	-DCONFIG_TCC_BACKTRACE=0 -DCONFIG_TCC_BCHECK=0 -DCONFIG_TCC_SEMLOCK=0 \
	-DCONFIG_TCCDIR='"/lib/tcc"' -DCONFIG_TCC_SYSINCLUDEPATHS='"/include:{B}/include"' \
	-DCONFIG_TCC_LIBPATHS='"{B}"' -DCONFIG_TCC_CRTPREFIX='"{B}"' -DCONFIG_TCC_ELFINTERP='"-"'
TCC_KOS    := $(TCC_OUT)/bin/tcc.elf

tcc-kos: $(TCC_KOS)

$(TCC_KOS): $(wildcard $(TCC_DIR)/*.c $(TCC_DIR)/*.h) $(TCC_LIBC_H) $(TCC_LIBS) $(TCC_CRT)
	@mkdir -p $(@D)
	$(TCC_X) $(TCC_CFLAGS) -I $(TCC_DIR) $(TCC_KOS_DEFS) -o $@ \
		$(TCC_OUT)/crt1.o $(TCC_DIR)/tcc.c $(TCC_LIBS)

# What goes on the disk for tcc, laid out as it is there: /bin/tcc
# (base64, like every /bin command), tcclibc's headers in /include, and
# tcc's own headers, libraries and start files in /lib/tcc. Made afresh each
# time, so a deleted header doesn't linger.
TCC_DISK := $(TCC_OUT)/disk

tcc-disk: $(TCC_KOS) $(TCC_LIBS) $(TCC_CRT)
	rm -rf $(TCC_DISK)
	mkdir -p $(TCC_DISK)/bin $(TCC_DISK)/lib/tcc/include
	base64 < $(TCC_KOS) | tr -d '\n' > $(TCC_DISK)/bin/tcc.elf.b64
	rsync -a --prune-empty-dirs --include='*/' --include='*.h' --exclude='*' userspace/tcclibc/ $(TCC_DISK)/include/
	cp $(TCC_DIR)/include/*.h $(TCC_DISK)/lib/tcc/include/
	cp $(TCC_LIBS) $(TCC_CRT) $(TCC_DISK)/lib/tcc/

FORCE:

# ---- disk ---------------------------------------------------------------
# disk.img is only created when missing, so `make run` never wipes what you
# wrote to it. `make disk` rebuilds it from scratch.

DISK_MIB := 32

$(DISK):
	$(MAKE) disk

disk: user games tcc-disk
	@# Leave out programs whose sources were deleted.
	rm -f $(filter-out $(USER_B64) $(CMD_B64) $(GAME_ELFS),$(wildcard $(USER_OUT)/disk/programs/* $(CMD_OUT)/disk/bin/* $(GAME_OUT)/disk/games/*))
	cargo run -q --release -p kos-mkdisk -- $(DISK) $(DISK_MIB) disk $(USER_OUT)/disk $(CMD_OUT)/disk $(GAME_OUT)/disk $(TCC_DISK)

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
