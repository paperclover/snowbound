// AppKit that winit, AccessKit and the app call and 10.6 lacks, added at load time with
// the neutral answer: scale 1, no precise scrolling, no appearances, no tabbing, no full
// screen. `remote.sh selectors` lists what a binary names that 10.6 lacks. Linked from
// the archive by the appearance name winit imports, so only AppKit binaries carry it.
// class_addMethod adds nothing where the system already has the method.
#include <ApplicationServices/ApplicationServices.h>
#include <objc/message.h>
#include <objc/runtime.h>

// 10.9's appearance name; it names the stub appearance below.
const CFStringRef NSAppearanceNameAqua = CFSTR("NSAppearanceNameAqua");

// Accessibility names from 10.7 to 10.13, which AccessKit imports. 10.6's VoiceOver
// asks through the older attribute protocol, which never sees them.
const CFStringRef NSAccessibilityAnnouncementKey = CFSTR("AXAnnouncementKey");
const CFStringRef NSAccessibilityAnnouncementRequestedNotification = CFSTR("AXAnnouncementRequested");
const CFStringRef NSAccessibilityLanguageTextAttribute = CFSTR("AXLanguageText");
const CFStringRef NSAccessibilityPriorityKey = CFSTR("AXPriorityKey");
const CFStringRef NSAccessibilitySwitchSubrole = CFSTR("AXSwitch");
const CFStringRef NSAccessibilityTabButtonSubrole = CFSTR("AXTabButton");
const CFStringRef NSAccessibilityTextAlignmentAttribute = CFSTR("AXTextAlignment");
const CFStringRef NSAccessibilityToggleSubrole = CFSTR("AXToggle");

extern void NSAccessibilityPostNotification(id element, CFStringRef notification);

void NSAccessibilityPostNotificationWithUserInfo(id element, CFStringRef notification, id info) {
    NSAccessibilityPostNotification(element, notification);
}

typedef id (*Send)(id, SEL);
#define SEND(receiver, selector) ((Send)objc_msgSend)((id)(receiver), sel_registerName(selector))
typedef double (*SendDouble)(id, SEL);

static double one(id self, SEL _cmd) { return 1.0; }
static void ignore(id self, SEL _cmd, id value) {}
static BOOL no(id self, SEL _cmd) { return NO; }
static long zero(id self, SEL _cmd) { return 0; }
static id nothing(id self, SEL _cmd) { return nil; }

static double delta_x(id self, SEL _cmd) { return ((SendDouble)objc_msgSend)(self, sel_registerName("deltaX")); }
static double delta_y(id self, SEL _cmd) { return ((SendDouble)objc_msgSend)(self, sel_registerName("deltaY")); }

// 10.7: the window's frame origin is where its base coordinates start.
static CGRect rect_to_screen(id self, SEL _cmd, CGRect rect) {
    CGRect frame = ((CGRect(*)(id, SEL))objc_msgSend_stret)(self, sel_registerName("frame"));
    rect.origin.x += frame.origin.x;
    rect.origin.y += frame.origin.y;
    return rect;
}

static CGPoint point_from_screen(id self, SEL _cmd, CGPoint point) {
    CGRect frame = ((CGRect(*)(id, SEL))objc_msgSend_stret)(self, sel_registerName("frame"));
    return CGPointMake(point.x - frame.origin.x, point.y - frame.origin.y);
}

static id srgb_color(id self, SEL _cmd, CGFloat red, CGFloat green, CGFloat blue, CGFloat alpha) {
    CGFloat components[] = {red, green, blue, alpha};
    return ((id (*)(id, SEL, id, CGFloat *, long))objc_msgSend)(
        self, sel_registerName("colorWithColorSpace:components:count:"),
        SEND(objc_getClass("NSColorSpace"), "sRGBColorSpace"), components, 4);
}

// 10.10's label is what 10.6 calls the description attribute.
static void set_accessibility_label(id self, SEL _cmd, id label) {
    ((void (*)(id, SEL, id, CFStringRef))objc_msgSend)(
        self, sel_registerName("accessibilitySetOverrideValue:forAttribute:"), label, CFSTR("AXDescription"));
}

static id appearance_class;

static id stub_appearance(id self, SEL _cmd) {
    static id shared;
    if (!shared) shared = SEND(SEND(appearance_class, "alloc"), "init");
    return shared;
}

static id aqua(id self, SEL _cmd) { return (id)NSAppearanceNameAqua; }
static id best_match(id self, SEL _cmd, id names) { return (id)NSAppearanceNameAqua; }

// 10.6 answers component accessors only for the named RGB spaces; later systems answer
// them for any RGB colour space, such as the sRGB the app converts system colours to.
static CGFloat component(id self, int index) {
    CGFloat components[4] = {0};
    ((void (*)(id, SEL, CGFloat *))objc_msgSend)(self, sel_registerName("getComponents:"), components);
    return components[index];
}
static CGFloat red(id self, SEL _cmd) { return component(self, 0); }
static CGFloat green(id self, SEL _cmd) { return component(self, 1); }
static CGFloat blue(id self, SEL _cmd) { return component(self, 2); }

static id black(id self, SEL _cmd) { return SEND(objc_getClass("NSColor"), "blackColor"); }
static id secondary_selection(id self, SEL _cmd) {
    return SEND(objc_getClass("NSColor"), "secondarySelectedControlColor");
}

#define RECT "{CGRect={CGPoint=dd}{CGSize=dd}}"

static void add(const char *class_name, int meta, const char *selector, void *imp, const char *types) {
    Class cls = (Class)objc_getClass(class_name);
    if (meta) cls = object_getClass((id)cls);
    class_addMethod(cls, sel_registerName(selector), (IMP)imp, types);
}

__attribute__((constructor)) static void polyfill(void) {
    add("NSScreen", 0, "backingScaleFactor", one, "d@:");
    add("NSWindow", 0, "backingScaleFactor", one, "d@:");
    add("NSWindow", 0, "convertRectToScreen:", rect_to_screen, RECT "@:" RECT);
    add("NSWindow", 0, "convertPointFromScreen:", point_from_screen, "{CGPoint=dd}@:{CGPoint=dd}");
    add("NSWindow", 0, "performWindowDragWithEvent:", ignore, "v@:@");
    add("NSWindow", 0, "toggleFullScreen:", ignore, "v@:@");
    add("NSWindow", 0, "setTabbingMode:", ignore, "v@:q");
    add("NSWindow", 0, "setTabbingIdentifier:", ignore, "v@:@");
    add("NSWindow", 1, "setAllowsAutomaticWindowTabbing:", ignore, "v@:c");
    add("NSWindow", 0, "setTitlebarAppearsTransparent:", ignore, "v@:c");
    add("NSWindow", 0, "setTitleVisibility:", ignore, "v@:q");
    add("NSView", 0, "setWantsBestResolutionOpenGLSurface:", ignore, "v@:c");
    add("NSView", 0, "setAccessibilityLabel:", set_accessibility_label, "v@:@");
    add("NSDatePicker", 0, "setPresentsCalendarOverlay:", ignore, "v@:c");
    add("NSEvent", 0, "hasPreciseScrollingDeltas", no, "c@:");
    add("NSEvent", 0, "scrollingDeltaX", delta_x, "d@:");
    add("NSEvent", 0, "scrollingDeltaY", delta_y, "d@:");
    add("NSEvent", 0, "momentumPhase", zero, "Q@:");
    add("NSEvent", 0, "phase", zero, "Q@:");
    add("NSColorSpaceColor", 0, "redComponent", red, "d@:");
    add("NSColorSpaceColor", 0, "greenComponent", green, "d@:");
    add("NSColorSpaceColor", 0, "blueComponent", blue, "d@:");
    add("NSColor", 1, "colorWithSRGBRed:green:blue:alpha:", srgb_color, "@@:dddd");
    add("NSColor", 1, "textInsertionPointColor", black, "@@:");
    add("NSColor", 1, "unemphasizedSelectedTextBackgroundColor", secondary_selection, "@@:");

    // One appearance, Aqua, which windows have and nothing changes.
    appearance_class = (id)objc_getClass("NSAppearance");
    if (!appearance_class) {
        Class cls = objc_allocateClassPair((Class)objc_getClass("NSObject"), "NSAppearance", 0);
        objc_registerClassPair(cls);
        appearance_class = (id)cls;
    }
    add("NSAppearance", 0, "name", aqua, "@@:");
    add("NSAppearance", 0, "bestMatchFromAppearancesWithNames:", best_match, "@@:@");
    add("NSAppearance", 1, "appearanceNamed:", nothing, "@@:@");
    add("NSAppearance", 1, "currentAppearance", nothing, "@@:");
    add("NSAppearance", 1, "setCurrentAppearance:", ignore, "v@:@");
    add("NSWindow", 0, "appearance", nothing, "@@:");
    add("NSWindow", 0, "setAppearance:", ignore, "v@:@");
    add("NSWindow", 0, "effectiveAppearance", stub_appearance, "@@:");
}
