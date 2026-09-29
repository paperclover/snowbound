// Writes the main display to a PNG, for a session reached over ssh, where
// screencapture exits 0 without writing a file. Wakes a sleeping display first: asleep,
// it shows its last frame.
#include <ApplicationServices/ApplicationServices.h>
#include <CoreServices/CoreServices.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: screen OUT.png\n");
        return 2;
    }
    UpdateSystemActivity(UsrActivity);
    sleep(2);
    CGImageRef image = CGDisplayCreateImage(CGMainDisplayID());
    if (!image) {
        fprintf(stderr, "screen: no image of the display\n");
        return 1;
    }
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)argv[1], strlen(argv[1]), false);
    CGImageDestinationRef file = CGImageDestinationCreateWithURL(url, CFSTR("public.png"), 1, NULL);
    CGImageDestinationAddImage(file, image, NULL);
    int ok = CGImageDestinationFinalize(file);
    return ok ? 0 : 1;
}
