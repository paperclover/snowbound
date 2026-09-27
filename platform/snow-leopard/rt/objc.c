// The ARC runtime entry points objc2 calls, which libobjc gained in 10.7 (objc_alloc in
// 10.9); on 10.6 they were compiled into each binary from libarclite. Linked from an
// archive, so only binaries that use Objective-C pull in libobjc.
#include <objc/message.h>
#include <objc/runtime.h>
#include <stddef.h>

typedef id (*Send)(id, SEL);
#define SEND(receiver, selector) ((Send)objc_msgSend)((receiver), sel_registerName(selector))

extern void *_Block_copy(const void *);

id objc_retain(id object) { return object ? SEND(object, "retain") : nil; }
void objc_release(id object) { if (object) SEND(object, "release"); }
id objc_autorelease(id object) { return object ? SEND(object, "autorelease") : nil; }
id objc_autoreleaseReturnValue(id object) { return objc_autorelease(object); }
id objc_retainAutorelease(id object) { return objc_autorelease(objc_retain(object)); }
id objc_retainAutoreleaseReturnValue(id object) { return objc_retainAutorelease(object); }
id objc_retainAutoreleasedReturnValue(id object) { return objc_retain(object); }
id objc_retainBlock(id block) { return (id)_Block_copy(block); }

void objc_storeStrong(id *location, id value) {
    id previous = *location;
    *location = objc_retain(value);
    objc_release(previous);
}

id objc_alloc(Class cls) { return cls ? SEND((id)cls, "alloc") : nil; }

id objc_allocWithZone(Class cls) {
    return cls ? ((id (*)(id, SEL, void *))objc_msgSend)((id)cls, sel_registerName("allocWithZone:"), NULL) : nil;
}

// Draining a pool also drains every pool pushed after it, as objc_autoreleasePoolPop does.
void *objc_autoreleasePoolPush(void) {
    return SEND(SEND((id)objc_getClass("NSAutoreleasePool"), "alloc"), "init");
}

void objc_autoreleasePoolPop(void *pool) { SEND((id)pool, "drain"); }
