# Sandbox

A falling-sand toy for KOS. Paint sand, water, oil, lava, fire and more with
the mouse, and watch them pour, pile up, float, burn and boil off:

- **Sand** falls and piles up, sinking through water and oil.
- **Water** and **oil** flow and level out; oil floats on water.
- **Stone**, **wood** and **ice** stay where you put them.
- **Fire** burns wood (slowly), plants and oil (fast), and goes out in water,
  boiling it to steam.
- **Lava** creeps, sets things alight, and turns water to steam and itself
  to stone.
- **Plants** grow into water; **ice** slowly freezes it.
- **Smoke** and **steam** rise and fade, and steam rains back down as water.

Left-drag paints and right-drag erases. Pick what to paint from the toolbar,
or with 1-9, 0 and -, and `e` for the eraser. The mouse wheel or `[` and `]`
size the brush. Space pauses, `c` clears, `s` saves the scene to
`/games/sandbox.sav` and `l` loads it back.

`make disk` in vm_os builds it (along with every other game in `games/`)
and puts it on the disk as `/games/sandbox.elf`; `make games` just builds it.
Then in the KOS shell:

    run /games/sandbox.elf

Escape quits.
