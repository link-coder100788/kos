"""lldb helpers for debugging KOS under QEMU.

    make debug        # terminal 1: KOS with QEMU's debug server
    make lldb         # terminal 2: lldb attached, with this loaded

Each QEMU CPU is an lldb thread: thread 1 is core 0, thread 2 core 1, and
so on. lldb's own commands (bt, frame variable, breakpoint set, step...)
work on whatever those cores are running. On top of that:

    kos tasks         every task: id, core, name, and where it is
    kos bt <id>       a task's backtrace, even if it's blocked
    kos prog <name>   load a program's symbols (from build/user), to debug it
    kos attach [port] connect to QEMU (`make lldb` does this)

Tasks that aren't running have their registers in the TrapFrame the
scheduler saved them in; KOS_DEBUG_TASKS (kernel/src/scheduler/mod.rs)
says where.
"""

import os
import struct
import subprocess
import sys

import lldb

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
KERNEL = os.path.join(ROOT, "target/aarch64-unknown-none/release/kos-kernel")
PROGRAM_DIRS = [os.path.join(ROOT, "build/user"), os.path.join(ROOT, "build/user/cmd")]

# Must match kernel/src/process.rs and kernel/src/scheduler.
USER_BASE = 0x5000_0000
DEBUG_TASK = struct.Struct("<4Q24s")  # id, core, frame, ttbr0, name
TRAP_FRAME_SIZE = 816
FRAME_ELR = 31 * 8
FRAME_SPSR = 32 * 8
FRAME_SP_EL0 = 36 * 8
MAX_FRAMES = 64

# The program whose symbols `kos prog` loaded.
loaded_program = None


def __lldb_init_module(debugger, _internal_dict):
    load_rust_formatters(debugger)
    debugger.HandleCommand('command container add -h "KOS helpers: kos help" kos')
    for name, function in [("tasks", "tasks"), ("bt", "backtrace"), ("prog", "program"),
                           ("attach", "attach"), ("help", "help_command")]:
        debugger.HandleCommand(f"command script add -o -f {__name__}.{function} kos {name}")


def load_rust_formatters(debugger):
    """Rust's own lldb formatters, from the toolchain this repo builds with
    (what `rust-lldb` loads): String, Vec, Option, Box, Rc and the rest show
    their contents instead of their insides."""
    try:
        sysroot = subprocess.run(["rustc", "--print", "sysroot"], cwd=ROOT, capture_output=True,
                                 text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        print("kos: rustc not found, so Rust types show raw")
        return
    lookup = os.path.join(sysroot, "lib/rustlib/etc/lldb_lookup.py")
    if not os.path.exists(lookup):
        return
    debugger.HandleCommand(f'command script import "{lookup}"')
    # In optimized code a variable's memory is often gone; the formatters
    # then throw (a Python traceback per variable). Show it as unavailable.
    module = sys.modules.get("lldb_lookup")
    for name in dir(module) if module else []:
        provider = getattr(module, name)
        if name.endswith("SummaryProvider") and callable(provider):
            setattr(module, name, forgiving(provider))
        elif name.endswith("SyntheticProvider") and isinstance(provider, type) and hasattr(provider, "num_children"):
            setattr(module, name, without_garbage(provider))


def without_garbage(provider):
    """A synthetic provider (a Vec's or a slice's elements...) that shows no
    elements when the first can't be read: the length it found is then
    garbage too, and would list thousands of failed reads."""
    class Checked(provider):
        def num_children(self, *args):
            try:
                n = super().num_children(*args)
                if n:
                    first = self.get_child_at_index(0)
                    if not first or not first.IsValid() or first.GetError().Fail() or first.GetData().GetByteSize() == 0:
                        return 0
                return n
            except Exception:
                return 0
    Checked.__name__ = provider.__name__
    return Checked


def forgiving(summary):
    def wrapped(valobj, internal_dict):
        try:
            return summary(valobj, internal_dict)
        except Exception:
            return "<unavailable>"
    wrapped.__name__ = summary.__name__
    return wrapped


def help_command(debugger, command, result, internal_dict):
    result.AppendMessage(__doc__.strip())


# ---- connecting ----------------------------------------------------------------

def attach(debugger, command, result, internal_dict):
    port = command.strip() or "1234"
    interpreter = debugger.GetCommandInterpreter()
    out = lldb.SBCommandReturnObject()
    interpreter.HandleCommand(f"gdb-remote {port}", out)
    if not out.Succeeded():
        result.SetError(f"couldn't connect to QEMU on port {port}: {out.GetError().strip()} (is `make debug` running?)")
        return
    # The kernel runs where it's linked: no slide.
    kernel = os.path.basename(debugger.GetSelectedTarget().GetExecutable().fullpath)
    interpreter.HandleCommand(f"target modules load --file {kernel} --slide 0", out)
    cores = debugger.GetSelectedTarget().GetProcess().GetNumThreads()
    result.AppendMessage(f"Attached to KOS, stopped. Threads 1-{cores} are cores 0-{cores - 1}. "
                         "`kos help` for KOS commands, `c` to continue.")
    threads = lldb.SBCommandReturnObject()
    interpreter.HandleCommand("thread list", threads)
    result.AppendMessage(threads.GetOutput().strip())


# ---- reading the kernel ----------------------------------------------------------

def global_address(target, name):
    symbols = target.FindSymbols(name)
    if not symbols.GetSize():
        return None
    return symbols.GetContextAtIndex(0).GetSymbol().GetStartAddress().GetLoadAddress(target)


def read(process, address, size):
    error = lldb.SBError()
    data = process.ReadMemory(address, size, error)
    return data if error.Success() else None


def read_u64(process, address):
    data = read(process, address, 8)
    return struct.unpack("<Q", data)[0] if data else None


def tasks_and_current(target, process):
    """The debugger table's tasks, and each core's running task id."""
    table = global_address(target, "KOS_DEBUG_TASKS")
    current_at = global_address(target, "KOS_DEBUG_CURRENT")
    if table is None or current_at is None:
        raise RuntimeError("no KOS_DEBUG_TASKS in this kernel: rebuild it (make kos)")
    size = target.FindSymbols("KOS_DEBUG_TASKS").GetContextAtIndex(0).GetSymbol().GetSize() or 512 * DEBUG_TASK.size
    data = read(process, table, size)
    if data is None:
        raise RuntimeError("couldn't read the task table")
    tasks = []
    for offset in range(0, len(data) - DEBUG_TASK.size + 1, DEBUG_TASK.size):
        task_id, core, frame, ttbr0, name = DEBUG_TASK.unpack_from(data, offset)
        if task_id:
            tasks.append({"id": task_id, "core": core, "frame": frame, "ttbr0": ttbr0,
                          "name": name.split(b"\0")[0].decode(errors="replace")})
    tasks.sort(key=lambda t: t["id"])
    cores = process.GetNumThreads()
    current = [read_u64(process, current_at + 8 * core) or 0 for core in range(cores)]
    return tasks, current


def describe(target, address):
    """`function+offset at file:line` for a code address, or just the hex."""
    resolved = target.ResolveLoadAddress(address)
    symbol = resolved.GetSymbol()
    if not symbol.IsValid():
        return f"{address:#x}"
    name = symbol.GetDisplayName() or symbol.GetName()
    offset = address - symbol.GetStartAddress().GetLoadAddress(target)
    text = f"{name}+{offset}" if offset else name
    line = resolved.GetLineEntry()
    if line.IsValid() and line.GetFileSpec().GetFilename():
        text += f" at {line.GetFileSpec().GetFilename()}:{line.GetLine()}"
    return text


def saved_registers(process, frame):
    """pc, sp, fp and lr from a TrapFrame, and whether it was in user mode."""
    data = read(process, frame, TRAP_FRAME_SIZE)
    if data is None:
        return None
    regs = struct.unpack_from("<31Q", data)
    pc, spsr = struct.unpack_from("<QQ", data, FRAME_ELR)
    user = spsr & 0xF == 0
    sp = struct.unpack_from("<Q", data, FRAME_SP_EL0)[0] if user else frame + TRAP_FRAME_SIZE
    return {"pc": pc, "sp": sp, "fp": regs[29], "lr": regs[30], "user": user}


def live_registers(process, core):
    thread = process.GetThreadAtIndex(core)
    frame = thread.GetFrameAtIndex(0)
    get = lambda name: frame.FindRegister(name).GetValueAsUnsigned()
    cpsr = get("cpsr")
    return {"pc": frame.GetPC(), "sp": frame.GetSP(), "fp": get("fp"), "lr": get("lr"), "user": cpsr & 0xF == 0}


def walk(process, regs):
    """Return addresses from the pc and the frame-pointer chain. The lr comes
    second when the function at pc hasn't saved it in a frame record yet (a
    leaf, or caught on its first instructions)."""
    addresses = [regs["pc"]]
    chain, fp, seen = [], regs["fp"], set()
    while fp and fp % 8 == 0 and fp not in seen and len(chain) < MAX_FRAMES:
        seen.add(fp)
        record = read(process, fp, 16)
        if record is None:
            break
        fp, ret = struct.unpack("<QQ", record)
        if ret:
            chain.append(ret)
    if regs["lr"] and (not chain or chain[0] != regs["lr"]):
        addresses.append(regs["lr"])
    return addresses + chain


# ---- commands --------------------------------------------------------------------

def tasks(debugger, command, result, internal_dict):
    target = debugger.GetSelectedTarget()
    process = target.GetProcess()
    try:
        all_tasks, current = tasks_and_current(target, process)
    except RuntimeError as e:
        result.SetError(str(e))
        return
    result.AppendMessage(f"{'ID':>4} {'CORE':>4}  {'STATE':<8} {'NAME':<20} WHERE")
    for task in all_tasks:
        running = task["core"] < len(current) and current[task["core"]] == task["id"]
        regs = live_registers(process, task["core"]) if running else saved_registers(process, task["frame"])
        where = describe(target, regs["pc"]) if regs else "?"
        if regs and regs["user"]:
            where += "  (program)"
        state = "running" if running else "stopped"
        result.AppendMessage(f"{task['id']:>4} {task['core']:>4}  {state:<8} {task['name']:<20} {where}")
    idle = [str(core) for core, task_id in enumerate(current) if task_id == 0]
    if idle:
        result.AppendMessage(f"Idle (or not running tasks): core {', '.join(idle)}")


def backtrace(debugger, command, result, internal_dict):
    target = debugger.GetSelectedTarget()
    process = target.GetProcess()
    try:
        task_id = int(command.strip(), 0)
    except ValueError:
        result.SetError("usage: kos bt <task id>  (ids are in `kos tasks`)")
        return
    try:
        all_tasks, current = tasks_and_current(target, process)
    except RuntimeError as e:
        result.SetError(str(e))
        return
    task = next((t for t in all_tasks if t["id"] == task_id), None)
    if task is None:
        result.SetError(f"no task {task_id}")
        return
    running = current[task["core"]] == task_id
    regs = live_registers(process, task["core"]) if running else saved_registers(process, task["frame"])
    if regs is None:
        result.SetError("couldn't read its saved registers")
        return
    how = f"running on core {task['core']} (thread {task['core'] + 1})" if running else "stopped"
    result.AppendMessage(f"task {task_id} '{task['name']}', {how}, {'in the program' if regs['user'] else 'in the kernel'}:")
    if regs["user"] and loaded_program != task["name"]:
        result.AppendMessage(f"  (`kos prog {task['name']}` for its function names)")
    for i, address in enumerate(walk(process, regs)):
        result.AppendMessage(f"  #{i:<2} {describe(target, address)}")
    if regs["user"] and not running:
        result.AppendMessage("  (a stopped program's stack is in its own address space, so it may end early)")


def program(debugger, command, result, internal_dict):
    global loaded_program
    name = command.strip()
    if not name:
        result.SetError("usage: kos prog <name>  (e.g. kos prog ksh)")
        return
    base = name.removesuffix(".b64").removesuffix(".elf")
    path = next((os.path.join(d, base + ".elf") for d in PROGRAM_DIRS
                 if os.path.exists(os.path.join(d, base + ".elf"))), None)
    if path is None:
        result.SetError(f"no {base}.elf in build/user or build/user/cmd (make user)")
        return
    target = debugger.GetSelectedTarget()
    # Every program is linked at 0 and loaded at USER_BASE, so only one
    # program's symbols can be loaded at a time.
    for i in reversed(range(target.GetNumModules())):
        module = target.GetModuleAtIndex(i)
        if module.GetFileSpec().fullpath and module.GetFileSpec().fullpath.startswith(tuple(PROGRAM_DIRS)):
            target.RemoveModule(module)
    module = target.AddModule(path, None, None)
    if not module.IsValid():
        result.SetError(f"lldb couldn't load {path}")
        return
    target.SetModuleLoadAddress(module, USER_BASE)
    loaded_program = base
    result.AppendMessage(f"Loaded {os.path.relpath(path, ROOT)} at {USER_BASE:#x}. "
                         f"Breakpoints in it hit in whichever program is at that address, "
                         f"so run one at a time.")
