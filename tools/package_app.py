#!/usr/bin/env python3
"""Packages KOS and QEMU into a double-clickable macOS app (`make package`).

    tools/package_app.py --qemu <qemu-system-aarch64> --kos build/kos.bin
                         --disk disk.img --out build/KOS.app

The app holds:

    Contents/MacOS/KOS                  the launcher (a shell script)
    Contents/MacOS/qemu-system-aarch64  QEMU, re-signed with its entitlements
    Contents/Frameworks/*.dylib         every library QEMU needs that macOS
                                        doesn't have, pointed at each other
    Contents/Resources/kos.bin          the bootloader with the kernel
    Contents/Resources/disk.img         a fresh disk, copied out on first run

QEMU's libraries come from wherever it was installed (Homebrew, usually):
each one's own libraries are followed too, and their load paths rewritten
to @executable_path/../Frameworks, so the app runs on a Mac without them.
Everything is signed ad hoc (Apple silicon refuses unsigned code), QEMU
keeping its com.apple.security.hypervisor entitlement for ACCEL=hvf.

The launcher keeps the disk (and the settings, kos.conf) in
~/Library/Application Support/KOS, so what you do in KOS outlives the app
being rebuilt. Its QEMU flags follow the Makefile's QEMU variable.
"""

import argparse
import hashlib
import os
import plistlib
import re
import shutil
import subprocess
import sys

FRAMEWORKS_REF = "@executable_path/../Frameworks"


def run(*cmd, capture=False):
    result = subprocess.run(cmd, check=True, text=True, capture_output=capture)
    return result.stdout if capture else None


# ---- libraries -----------------------------------------------------------------

def is_system(path):
    """Libraries every Mac has, which stay where they are."""
    return path.startswith("/usr/lib/") or path.startswith("/System/")


def rpaths(binary):
    """A binary's LC_RPATH entries, with @loader_path worked out."""
    out = run("otool", "-l", binary, capture=True)
    found = re.findall(r"cmd LC_RPATH\n\s+cmdsize \d+\n\s+path (.+?) \(offset", out)
    here = os.path.dirname(binary)
    return [p.replace("@loader_path", here).replace("@executable_path", here) for p in found]


def dependencies(binary):
    """The libraries a binary loads (its install names), minus its own id."""
    lines = run("otool", "-L", binary, capture=True).splitlines()[1:]
    names = [line.strip().split(" (compatibility")[0] for line in lines if line.strip()]
    own_id = run("otool", "-D", binary, capture=True).splitlines()[1:]
    return [n for n in names if n not in own_id]


def locate(name, binary):
    """Where install name `name`, as `binary` loads it, really is."""
    here = os.path.dirname(binary)
    if name.startswith("@rpath/"):
        for base in rpaths(binary):
            candidate = os.path.join(base, name[len("@rpath/"):])
            if os.path.exists(candidate):
                return os.path.realpath(candidate)
        sys.exit(f"package: can't find {name} (needed by {binary}) on its rpath")
    if name.startswith("@loader_path/") or name.startswith("@executable_path/"):
        return os.path.realpath(os.path.join(here, name.split("/", 1)[1]))
    return os.path.realpath(name)


# Where each bundled library was copied from (None for QEMU itself).
origin = {}


def bundle_libraries(bundled_qemu, real_qemu, frameworks):
    """Copies every non-system library QEMU needs, directly or not, into
    frameworks. Each copy's references are resolved as the original's (its
    rpaths point into its own install). Returns {bundled binary: [(install
    name in it, bundled file name)]}, for rewriting."""
    rewrites = {}
    copied = {}  # real path -> bundled file name
    todo = [(bundled_qemu, real_qemu)]
    while todo:
        binary, real_binary = todo.pop()
        changes = []
        for name in dependencies(binary):
            if is_system(name):
                continue
            real = locate(name, real_binary)
            if real not in copied:
                file_name = os.path.basename(real)
                if file_name in copied.values():
                    sys.exit(f"package: two different libraries are both called {file_name}")
                dest = os.path.join(frameworks, file_name)
                shutil.copy2(real, dest)
                os.chmod(dest, 0o755)
                copied[real] = file_name
                origin[dest] = real
                todo.append((dest, real))
            changes.append((name, copied[real]))
        rewrites[binary] = changes
    return rewrites


def fix_load_paths(rewrites):
    for binary, changes in rewrites.items():
        args = []
        if origin.get(binary):
            args += ["-id", f"{FRAMEWORKS_REF}/{os.path.basename(binary)}"]
        for old, new in changes:
            args += ["-change", old, f"{FRAMEWORKS_REF}/{new}"]
        if args:
            # The changes invalidate any signature; everything is re-signed after.
            run("install_name_tool", *args, binary, capture=True)


# ---- signing ---------------------------------------------------------------------

def entitlements(binary, out_path):
    """Writes binary's entitlements (as a plist) to out_path. Returns
    whether it had any."""
    xml = subprocess.run(["codesign", "-d", "--entitlements", "-", "--xml", binary],
                         capture_output=True).stdout
    start = xml.find(b"<?xml")
    if start < 0:
        return False
    with open(out_path, "wb") as f:
        f.write(xml[start:])
    return True


def sign(path, entitlements_path=None):
    cmd = ["codesign", "--force", "--sign", "-", "--timestamp=none"]
    if entitlements_path:
        cmd += ["--entitlements", entitlements_path]
    run(*cmd, path, capture=True)


# ---- the app -----------------------------------------------------------------------

LAUNCHER = r'''#!/bin/bash
# KOS.app's launcher: boots KOS in the QEMU next to it. Made by
# tools/package_app.py; the flags follow the Makefile's QEMU variable.

here="$(cd "$(dirname "$0")" && pwd)"
resources="$here/../Resources"
data="$HOME/Library/Application Support/KOS"
disk="$data/disk.img"
mkdir -p "$data"

alert() {
    osascript -e 'on run argv' -e 'display alert "KOS" message (item 1 of argv) as critical' -e 'end run' "$1" >/dev/null
}

if [ ! -f "$data/kos.conf" ]; then
    cat > "$data/kos.conf" <<'CONF'
# KOS.app's settings (a shell file, read each time it starts).
# ACCEL=hvf runs KOS on the Mac's CPU directly: much faster, but there are
# no random numbers under it yet, so HTTPS and sshd don't work.
ACCEL=tcg
SMP=5
MEM=512M
# Ports on this Mac passed through to KOS, as QEMU hostfwd rules separated
# by spaces. 127.0.0.1 keeps them to this Mac: ssh -p 2222 me@localhost
# reaches sshd (start it in KOS with `bg sshd`).
FWD="tcp:127.0.0.1:2222-:22"
# The display device: virtio-gpu or ramfb.
DISPLAY_DEV=virtio-gpu
# 1 starts KOS full screen. In a window, resizing it scales KOS to fit.
FULL_SCREEN=0
# Log filters, e.g. LOG="net=debug" (see the README's Logging section).
LOG=
CONF
fi
ACCEL=tcg SMP=5 MEM=512M FWD="" DISPLAY_DEV=virtio-gpu FULL_SCREEN=0 LOG=
. "$data/kos.conf"

# The disk lives here, out of the app, so it outlives a rebuilt app. A
# rebuilt app may bring a newer disk (new programs): offer to switch.
packaged_id="$(cat "$resources/disk.id")"
if [ ! -f "$disk" ]; then
    cp "$resources/disk.img" "$disk" && echo "$packaged_id" > "$data/disk.id"
elif [ "$(cat "$data/disk.id" 2>/dev/null)" != "$packaged_id" ]; then
    answer="$(osascript -e 'display dialog "This copy of KOS comes with a newer disk (new or updated programs). Use it? Your current disk is kept as disk.img.old, in ~/Library/Application Support/KOS." buttons {"Keep My Disk", "Use the New Disk"} default button "Keep My Disk" with title "KOS"' -e 'button returned of result' 2>/dev/null)"
    if [ "$answer" = "Use the New Disk" ]; then
        mv "$disk" "$disk.old" && cp "$resources/disk.img" "$disk"
    fi
    echo "$packaged_id" > "$data/disk.id"
fi

# QEMU locks the disk, so a second KOS would only fail.
if lsof -t "$disk" >/dev/null 2>&1; then
    alert "KOS is already running (its disk is in use)."
    exit 0
fi

cpu=max
[ "$ACCEL" = hvf ] && cpu=host
display_device=virtio-gpu-device
[ "$DISPLAY_DEV" = ramfb ] && display_device=ramfb
netdev="user,id=net0"
for rule in $FWD; do netdev="$netdev,hostfwd=$rule"; done
extra=()
[ "$FULL_SCREEN" = 1 ] && extra+=(-full-screen)
log=()
[ -n "$LOG" ] && log=(-fw_cfg "name=opt/kos/log,string=${LOG//,/,,}")

# QEMU's messages go to a log, since there's no terminal to show them.
exec "$here/qemu-system-aarch64" \
    -name KOS \
    -machine virt,secure=off,highmem=off -accel "$ACCEL" -cpu "$cpu" -smp "$SMP" -m "$MEM" \
    -bios "$resources/kos.bin" \
    -global virtio-mmio.force-legacy=false \
    -drive "file=$disk,if=none,format=raw,id=hd0" -device virtio-blk-device,drive=hd0 \
    -device virtio-keyboard-device -device virtio-tablet-device \
    -netdev "$netdev" -device virtio-net-device,netdev=net0 \
    -audiodev coreaudio,id=snd0 -device virtio-sound-device,audiodev=snd0,streams=1 \
    -rtc base=utc -fw_cfg "name=opt/kos/utc_offset,string=$(date +%z)" \
    "${log[@]}" "${extra[@]}" \
    -device "$display_device" -display cocoa,zoom-to-fit=on -serial vc \
    2> "$data/qemu.log"
'''


def info_plist(version):
    return {
        "CFBundleName": "KOS",
        "CFBundleDisplayName": "KOS",
        "CFBundleIdentifier": "local.kos.KOS",
        "CFBundleExecutable": "KOS",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "CFBundleInfoDictionaryVersion": "6.0",
        "LSMinimumSystemVersion": "12.0",
        "LSApplicationCategoryType": "public.app-category.developer-tools",
        "NSHighResolutionCapable": True,
        "NSHumanReadableCopyright": "KOS, a hobby operating system, running in QEMU (GPL-2.0).",
    }


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--qemu", required=True, help="qemu-system-aarch64 to bundle")
    parser.add_argument("--kos", required=True, help="build/kos.bin")
    parser.add_argument("--disk", required=True, help="the disk image to start from")
    parser.add_argument("--out", required=True, help="the .app to make (replaced if it exists)")
    parser.add_argument("--version", default="0.1.0")
    args = parser.parse_args()

    qemu = shutil.which(args.qemu) or args.qemu
    qemu = os.path.realpath(qemu)
    if not os.path.isfile(qemu):
        sys.exit(f"package: no QEMU at {args.qemu} (brew install qemu, or pass QEMU_BIN=...)")
    for path in (args.kos, args.disk):
        if not os.path.isfile(path):
            sys.exit(f"package: {path} is missing")

    app = args.out
    if os.path.exists(app):
        shutil.rmtree(app)
    macos = os.path.join(app, "Contents", "MacOS")
    frameworks = os.path.join(app, "Contents", "Frameworks")
    resources = os.path.join(app, "Contents", "Resources")
    for d in (macos, frameworks, resources):
        os.makedirs(d)

    bundled_qemu = os.path.join(macos, "qemu-system-aarch64")
    shutil.copy2(qemu, bundled_qemu)
    os.chmod(bundled_qemu, 0o755)
    origin[bundled_qemu] = None  # not a library: no id to set

    print(f"package: bundling {qemu} and its libraries")
    rewrites = bundle_libraries(bundled_qemu, qemu, frameworks)
    fix_load_paths(rewrites)

    shutil.copy2(args.kos, os.path.join(resources, "kos.bin"))
    shutil.copy2(args.disk, os.path.join(resources, "disk.img"))
    with open(os.path.join(resources, "disk.id"), "w") as f:
        f.write(sha256_file(args.disk) + "\n")
    launcher = os.path.join(macos, "KOS")
    with open(launcher, "w") as f:
        f.write(LAUNCHER)
    os.chmod(launcher, 0o755)
    with open(os.path.join(app, "Contents", "Info.plist"), "wb") as f:
        plistlib.dump(info_plist(args.version), f)

    # Inside out: the libraries, QEMU (with its entitlements), then the app.
    for name in sorted(os.listdir(frameworks)):
        sign(os.path.join(frameworks, name))
    ent = os.path.join(os.path.dirname(app), "qemu-entitlements.plist")
    sign(bundled_qemu, ent if entitlements(qemu, ent) else None)
    if os.path.exists(ent):
        os.remove(ent)
    sign(app)

    size = sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(app) for f in fs)
    print(f"package: made {app} ({len(os.listdir(frameworks))} libraries, {size / 1e6:.0f} MB)")


if __name__ == "__main__":
    main()
