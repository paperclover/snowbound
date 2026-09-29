// Reads selector names on stdin and prints those no class on this system implements,
// instance or class side: calls a 10.6 binary could make that would raise at run time.
// `strings BINARY` supplies candidates; names that were never selectors print too.
// Method lists are read directly: asking a class resolves methods, which some raise.
#include <dlfcn.h>
#include <objc/runtime.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int compare(const void *a, const void *b) {
    return strcmp(*(const char *const *)a, *(const char *const *)b);
}

int main(void) {
    const char *frameworks[] = {"AppKit", "Carbon", "QuartzCore", "Security", "OpenGL"};
    for (size_t i = 0; i < sizeof frameworks / sizeof *frameworks; i++) {
        char path[256];
        snprintf(path, sizeof path, "/System/Library/Frameworks/%s.framework/%s", frameworks[i], frameworks[i]);
        dlopen(path, RTLD_NOW);
    }
    int count = objc_getClassList(NULL, 0);
    Class *classes = malloc(sizeof(Class) * count);
    objc_getClassList(classes, count);
    size_t names = 0, capacity = 1 << 16;
    const char **known = malloc(sizeof(char *) * capacity);
    for (int i = 0; i < count * 2; i++) {
        Class cls = i < count ? classes[i] : object_getClass((id)classes[i - count]);
        unsigned methods = 0;
        Method *list = class_copyMethodList(cls, &methods);
        for (unsigned m = 0; m < methods; m++) {
            if (names == capacity) known = realloc(known, sizeof(char *) * (capacity *= 2));
            known[names++] = sel_getName(method_getName(list[m]));
        }
        free(list);
    }
    qsort(known, names, sizeof(char *), compare);
    char line[1024];
    while (fgets(line, sizeof line, stdin)) {
        line[strcspn(line, "\n")] = 0;
        const char *name = line;
        if (!bsearch(&name, known, names, sizeof(char *), compare)) puts(line);
    }
    return 0;
}
