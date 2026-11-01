module song;

import kernel;

fn main() {
    kernel.print("A scale, a tone, a beep and a tune\n");

    mut i: u64 = 0;
    let steps: u64[8] = [262, 294, 330, 349, 392, 440, 494, 523];
    while i < 8 {
        kernel.snd(steps[i]);
        kernel.sleep_ms(150);
        i++;
    }
    kernel.snd(0);
    kernel.sleep_ms(200);

    kernel.beep();
    kernel.play("T160 O4 E8 E8 R8 E8 R8 C8 E4 G4 R4 < G4");
    kernel.exit(0);
}
