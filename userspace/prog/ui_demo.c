// Opens a window with a counter and buttons, and reacts to clicks until
// the window is closed.
#include "../libc/sys.h"
#include "../libc/print/print.h"

void _start(void) {
    ui_element_t elements[] = {
        ui_label(16, 16, "Hello from a C program!", 0x20, 0x20, 0x20, 2),
        ui_button(16, 56, 120, 32, "Say hi"),
        ui_button(152, 56, 120, 32, "Save file"),
        ui_button(288, 56, 120, 32, "Quit"),
    };
    long id = ui_create_window("UI Demo", elements, 4);
    printf("ui_demo: opened window %ld\n", id);

    for (;;) {
        long event = ui_poll_event(id);
        if (event == 2) {
            writeln("ui_demo: hi!");
        } else if (event == 3) {
            const char* text = "written by ui_demo\n";
            size_t len = 0;
            while (text[len]) len++;
            long r = fs_write("ui_demo.txt", text, len);
            char back[64];
            long n = fs_read("ui_demo.txt", back, sizeof(back) - 1);
            if (r == 0 && n > 0) {
                back[n] = '\0';
                printf("ui_demo: saved and read back: %s", back);
            } else {
                writeln("ui_demo: file write failed");
            }
        } else if (event == 4 || event == -1) {
            ui_close_window(id);
            writeln("ui_demo: bye");
            exit();
        } else {
            sleep_ms(20);
        }
    }
}
