use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::NSCopying;
use objc2_foundation::{NSObject, NSString};

fn main() {
    autoreleasepool(|_| {
        let text = NSString::from_str("objc2 on Snow Leopard");
        let copy: Retained<NSString> = text.copy();
        let object = NSObject::new();
        println!("{copy} ({} chars), {object:?}", copy.length());
    });
}
