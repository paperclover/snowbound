// Posts input to the logged-in session as the HID system would, for driving the app
// over ssh: `input click X Y`, `input double X Y`, `input drag X Y X2 Y2`,
// `input scroll X Y LINES`, `input type TEXT`,
// `input key KEYCODE [command] [shift] [option] [control]`.
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

static void mouse(CGEventType type, CGPoint at, int clicks) {
    CGEventRef event = CGEventCreateMouseEvent(NULL, type, at, kCGMouseButtonLeft);
    CGEventSetIntegerValueField(event, kCGMouseEventClickState, clicks);
    post(event);
}

static void click(CGPoint at, int clicks) {
    mouse(kCGEventLeftMouseDown, at, clicks);
    mouse(kCGEventLeftMouseUp, at, clicks);
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
    if ((argc == 4 && !strcmp(argv[1], "click")) || (argc == 4 && !strcmp(argv[1], "double"))) {
        CGPoint at = CGPointMake(atof(argv[2]), atof(argv[3]));
        mouse(kCGEventMouseMoved, at, 0);
        click(at, 1);
        if (!strcmp(argv[1], "double")) click(at, 2);
    } else if (argc == 6 && !strcmp(argv[1], "drag")) {
        CGPoint from = CGPointMake(atof(argv[2]), atof(argv[3]));
        CGPoint to = CGPointMake(atof(argv[4]), atof(argv[5]));
        mouse(kCGEventMouseMoved, from, 0);
        mouse(kCGEventLeftMouseDown, from, 1);
        for (int step = 1; step <= 10; step++) {
            CGPoint at = CGPointMake(from.x + (to.x - from.x) * step / 10, from.y + (to.y - from.y) * step / 10);
            mouse(kCGEventLeftMouseDragged, at, 1);
        }
        mouse(kCGEventLeftMouseUp, to, 1);
    } else if (argc == 5 && !strcmp(argv[1], "scroll")) {
        CGPoint at = CGPointMake(atof(argv[2]), atof(argv[3]));
        mouse(kCGEventMouseMoved, at, 0);
        CGEventRef event = CGEventCreateScrollWheelEvent(NULL, kCGScrollEventUnitLine, 1, atoi(argv[4]));
        CGEventSetLocation(event, at);
        post(event);
    } else if (argc == 3 && !strcmp(argv[1], "type")) {
        CFStringRef text = CFStringCreateWithCString(NULL, argv[2], kCFStringEncodingUTF8);
        for (CFIndex i = 0; i < CFStringGetLength(text); i++) {
            UniChar c = CFStringGetCharacterAtIndex(text, i);
            key(0, 0, &c, 1);
        }
        CFRelease(text);
    } else if (argc >= 3 && !strcmp(argv[1], "key")) {
        CGEventFlags flags = 0;
        for (int i = 3; i < argc; i++) {
            if (!strcmp(argv[i], "command")) flags |= kCGEventFlagMaskCommand;
            else if (!strcmp(argv[i], "shift")) flags |= kCGEventFlagMaskShift;
            else if (!strcmp(argv[i], "option")) flags |= kCGEventFlagMaskAlternate;
            else if (!strcmp(argv[i], "control")) flags |= kCGEventFlagMaskControl;
        }
        key(atoi(argv[2]), flags, NULL, 0);
    } else {
        fprintf(stderr, "usage: input click|double X Y | drag X Y X2 Y2 | scroll X Y LINES | type TEXT | key KEYCODE [command] [shift] [option] [control]\n");
        return 2;
    }
    return 0;
}
