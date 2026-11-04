# Glowfly

A one-button game for KOS: guide a firefly through a forest at night. Space,
Up or a click makes it flutter; vines hang from above, thorns grow from below,
and the gaps between them sway. Your glow fades as you fly and the forest past
it is dark, so catch spores to keep your light up.

`make disk` in vm_os builds it (along with every other game in `games/`)
and puts it on the disk as `/games/glowfly.elf`; `make games` just builds it.
Then in the KOS shell:

    run /games/glowfly.elf

Escape quits. The best score is saved in `/glowfly.hi`.
