#![no_main]

use libfuzzer_sys::fuzz_target;
use notebook::live::wire;

fuzz_target!(|bytes: &[u8]| {
    macro_rules! decode {
        ($($message:ident),+) => {
            $(let _ = minicbor::decode::<wire::$message>(bytes);)+
        };
    }
    decode!(
        Hello, Presence, Bye, Welcome, Approval, Touched, Delta, Written, Request, Reply,
        WireStamp, WireEntry
    );
});
