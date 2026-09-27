// CoreText functions fontique calls that arrived after 10.6.
#include <ApplicationServices/ApplicationServices.h>

// 10.9: fallback font for a string in a language; 10.6 can only ignore the language.
CTFontRef CTFontCreateForStringWithLanguage(CTFontRef font, CFStringRef string, CFRange range,
                                            CFStringRef language) {
    (void)language;
    return CTFontCreateForString(font, string, range);
}
