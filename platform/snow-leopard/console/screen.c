// Writes the main display, or with OWNER the first window of the app of that name with
// its shadow on transparency, to a PNG, for a session reached over ssh, where
// screencapture exits 0 without writing a file. Wakes a sleeping display first: asleep,
// it shows its last frame.
#include <ApplicationServices/ApplicationServices.h>
#include <CoreServices/CoreServices.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static CGWindowID window_of(const char *owner) {
    CFArrayRef windows = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
    CFStringRef name = CFStringCreateWithCString(NULL, owner, kCFStringEncodingUTF8);
    CGWindowID found = 0;
    for (CFIndex i = 0; i < CFArrayGetCount(windows) && !found; i++) {
        CFDictionaryRef window = CFArrayGetValueAtIndex(windows, i);
        CFStringRef of = CFDictionaryGetValue(window, kCGWindowOwnerName);
        CFNumberRef layer = CFDictionaryGetValue(window, kCGWindowLayer);
        int level = -1;
        if (layer) CFNumberGetValue(layer, kCFNumberIntType, &level);
        if (of && level == 0 && CFStringCompare(of, name, 0) == kCFCompareEqualTo)
            CFNumberGetValue(CFDictionaryGetValue(window, kCGWindowNumber), kCFNumberIntType, &found);
    }
    CFRelease(name);
    CFRelease(windows);
    return found;
}

int main(int argc, char **argv) {
    if (argc != 2 && argc != 3) {
        fprintf(stderr, "usage: screen OUT.png [OWNER]\n");
        return 2;
    }
    UpdateSystemActivity(UsrActivity);
    sleep(2);
    CGImageRef image;
    if (argc == 3) {
        CGWindowID window = window_of(argv[2]);
        if (!window) {
            fprintf(stderr, "screen: no window of %s\n", argv[2]);
            return 1;
        }
        image = CGWindowListCreateImage(CGRectNull, kCGWindowListOptionIncludingWindow, window,
                                        kCGWindowImageDefault);
    } else {
        image = CGDisplayCreateImage(CGMainDisplayID());
    }
    if (!image) {
        fprintf(stderr, "screen: no image\n");
        return 1;
    }
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)argv[1], strlen(argv[1]), false);
    CGImageDestinationRef file = CGImageDestinationCreateWithURL(url, CFSTR("public.png"), 1, NULL);
    CGImageDestinationAddImage(file, image, NULL);
    int ok = CGImageDestinationFinalize(file);
    return ok ? 0 : 1;
}
