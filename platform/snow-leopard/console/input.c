// Posts input to the logged-in session as the HID system would, for driving the app
// over ssh: `input click X Y`, `input type TEXT`, `input key KEYCODE [command]`.
// Points are the display's, from its top left.
#include <ApplicationServices/ApplicationServices.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void post(CGEventRef event) {
    CGEventPost(kCGHIDEventTap, event);
    CFRelease(event);
    usleep(20000);
}

static void mouse(CGEventType type, CGPoint at) {
    post(CGEventCreateMouseEvent(NULL, type, at, kCGMouseButtonLeft));
}

static void key(CGKeyCode code, CGEventFlags flags, const UniChar *text, UniCharCount length) {
    for (int down = 1; down >= 0; down--) {
        CGEventRef event = CGEventCreateKeyboardEvent(NULL, code, down);
        CGEventSetFlags(event, flags);
        if (length) CGEventKeyboardSetUnicodeString(event, length, text);
        post(event);
    }
}

int main(int argc, char **argv) {
    if (argc == 4 && !strcmp(argv[1], "click")) {
        CGPoint at = CGPointMake(atof(argv[2]), atof(argv[3]));
        mouse(kCGEventMouseMoved, at);
        mouse(kCGEventLeftMouseDown, at);
        mouse(kCGEventLeftMouseUp, at);
    } else if (argc == 3 && !strcmp(argv[1], "type")) {
        CFStringRef text = CFStringCreateWithCString(NULL, argv[2], kCFStringEncodingUTF8);
        for (CFIndex i = 0; i < CFStringGetLength(text); i++) {
            UniChar c = CFStringGetCharacterAtIndex(text, i);
            key(0, 0, &c, 1);
        }
        CFRelease(text);
    } else if ((argc == 3 || argc == 4) && !strcmp(argv[1], "key")) {
        key(atoi(argv[2]), argc == 4 && !strcmp(argv[3], "command") ? kCGEventFlagMaskCommand : 0, NULL, 0);
    } else {
        fprintf(stderr, "usage: input click X Y | input type TEXT | input key KEYCODE [command]\n");
        return 2;
    }
    return 0;
}
