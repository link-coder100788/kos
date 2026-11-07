// Sound: one voice, like a PC speaker. snd() sets a tone that plays until
// the next sound; play() and beep() wait until they're done.
#include "../libc/sys.h"
#include "../libc/stdio.h"

void _start(void) {
    if (beep() < 0) {
        printf("no sound: %s\n", last_error());
        exit(0);
    }
    // A siren: sweep a tone up and down.
    for (int round = 0; round < 2; round++) {
        for (uint32_t hz = 400; hz < 800; hz += 10) { snd(hz); sleep_ms(10); }
        for (uint32_t hz = 800; hz > 400; hz -= 10) { snd(hz); sleep_ms(10); }
    }
    snd(0);
    sleep_ms(300);
    printf("playing a tune\n");
    if (play("T120 O4 C8 D8 E8 F8 G4 G4 A8 A8 A8 A8 G2 F8 F8 F8 F8 E4 E4 D8 D8 D8 D8 C2") < 0) {
        printf("play failed\n");
    }
    exit(0);
}
