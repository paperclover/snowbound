// Renders the pieces of 10.6's chrome that the Snow Leopard `--screenshot` samples, into
// DIR as PNGs: the window buttons, and scrollers with their knob and without, each for a
// key window and for another. `remote.sh art` fetches them into crates/snowbound/assets/snow-leopard.
#import <AppKit/AppKit.h>
#import <objc/runtime.h>

@interface KeyWindow : NSWindow
@end
@implementation KeyWindow
- (BOOL)isKeyWindow { return YES; }
- (BOOL)isMainWindow { return YES; }
@end

// A scroller's track and arrows alone.
@interface Track : NSScroller
@end
@implementation Track
- (void)drawKnob {}
@end

static NSString *dir;

static void save(NSView *view, NSRect rect, NSString *name) {
    NSBitmapImageRep *rep = [view bitmapImageRepForCachingDisplayInRect:rect];
    [view cacheDisplayInRect:rect toBitmapImageRep:rep];
    [[rep representationUsingType:NSPNGFileType properties:nil]
        writeToFile:[dir stringByAppendingPathComponent:name] atomically:YES];
}

static NSWindow *window(BOOL key) {
    NSWindow *window = [[(key ? [KeyWindow class] : [NSWindow class]) alloc]
        initWithContentRect:NSMakeRect(-10000, -10000, 500, 500)
                  styleMask:NSBorderlessWindowMask
                    backing:NSBackingStoreBuffered
                      defer:NO];
    [window setOpaque:NO];
    [window setBackgroundColor:[NSColor clearColor]];
    return window;
}

int main(int argc, char **argv) {
    NSAutoreleasePool *pool = [NSAutoreleasePool new];
    if (argc != 2) {
        fprintf(stderr, "usage: art DIR\n");
        return 2;
    }
    [NSApplication sharedApplication];
    dir = [NSString stringWithUTF8String:argv[1]];
    [[NSFileManager defaultManager] createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:nil error:NULL];
    NSUInteger style = NSTitledWindowMask | NSClosableWindowMask | NSMiniaturizableWindowMask
        | NSResizableWindowMask | NSTexturedBackgroundWindowMask;
    for (int key = 0; key < 2; key++) {
        NSString *state = key ? @"key" : @"other";
        NSView *content = [window(key) contentView];
        // Close, minimize and zoom, side by side at their own size.
        CGFloat x = 0, height = 0;
        for (NSWindowButton kind = NSWindowCloseButton; kind <= NSWindowZoomButton; kind++) {
            NSButton *button = [NSWindow standardWindowButton:kind forStyleMask:style];
            [button setFrameOrigin:NSMakePoint(x, 0)];
            [content addSubview:button];
            x += [button frame].size.width;
            height = [button frame].size.height;
        }
        save(content, NSMakeRect(0, 0, x, height), [NSString stringWithFormat:@"buttons-%@.png", state]);
        [[content subviews] makeObjectsPerformSelector:@selector(removeFromSuperview)];
        for (int vertical = 0; vertical < 2; vertical++) {
            NSString *axis = vertical ? @"vertical" : @"horizontal";
            CGFloat width = [NSScroller scrollerWidth];
            NSRect frame = vertical ? NSMakeRect(0, 0, width, 400) : NSMakeRect(0, 0, 400, width);
            NSScroller *track = [[Track alloc] initWithFrame:frame];
            [track setEnabled:YES];
            [track setKnobProportion:0.25];
            [content addSubview:track];
            save(track, [track bounds], [NSString stringWithFormat:@"scroller-%@-track-%@.png", axis, state]);
            [track removeFromSuperview];
            NSScroller *scroller = [[NSScroller alloc] initWithFrame:frame];
            [scroller setEnabled:YES];
            [scroller setKnobProportion:0.25];
            [scroller setDoubleValue:0.5];
            [content addSubview:scroller];
            save(scroller, [scroller bounds], [NSString stringWithFormat:@"scroller-%@-%@.png", axis, state]);
            [scroller removeFromSuperview];
        }
    }
    [pool drain];
    return 0;
}
