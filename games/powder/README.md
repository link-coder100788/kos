# Powder Run

A 3D downhill ski game for KOS, in the style of an early-80s arcade cabinet.
Race down an endless run against the clock: ski between the flags of each
gate to win time and build a combo, and dodge the trees and rocks. Left and
right (or A and D, or the mouse) turn your skis, Down points them straight
downhill and Up turns them across the slope to stop. Space or a click jumps,
which clears rocks but not trees.

Everything is drawn by a small software renderer in `powder.c`: flat-shaded
polygons clipped to the near plane and filled into a 320x240 buffer, which is
doubled up to the window with dimmed scanlines.

`make disk` in vm_os builds it (along with every other game in `games/`)
and puts it on the disk as `/games/powder.elf`; `make games` just builds it.
Then in the KOS shell:

    run /games/powder.elf

Escape quits. The best score is saved in `/powder.hi`.
