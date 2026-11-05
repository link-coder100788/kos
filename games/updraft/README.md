# Updraft

A 3D paper glider game for KOS. Glide down a winding desert canyon at dusk,
trading height for speed: diving speeds you up, climbing slows you down,
and if you get too slow you stall and the nose drops until you've picked up
speed again.

The mouse flies the glider: left and right bank it into a turn, up and down
point the nose. The arrow keys (or WASD) do the same a step at a time. Fly
through the rings for points and a burst of speed (each one in a row raises
your combo), ride the thermals (the rising dust) for height, and duck under
or hop over the stone arches. Touching rock ends the flight.

It's drawn with libc's `render3d.h` at 320x240, doubled up to the window.

`make disk` in vm_os builds it (along with every other game in `games/`)
and puts it on the disk as `/games/updraft.elf`; `make games` just builds it.
Then in the KOS shell:

    run /games/updraft.elf

Escape quits. The best score is saved in `/games/updraft.hi`.
