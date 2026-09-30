use std::cell::Cell;
use std::time::{Instant, SystemTime};

thread_local! {
    static COUNTER: Cell<u32> = const { Cell::new(0) };
}

fn main() {
    let dir = std::env::temp_dir().join(format!("snow-leopard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.txt");
    std::fs::write(&file, "hello from rust").unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let meta = std::fs::metadata(&file).unwrap();
    let entries = std::fs::read_dir(&dir).unwrap().count();
    // 10.6 packs directory entries 4 bytes apart; debug builds check them for alignment.
    let frameworks = std::fs::read_dir("/System/Library/Frameworks").map_or(0, Iterator::count);
    std::fs::remove_dir_all(&dir).unwrap();

    let start = Instant::now();
    let threads: Vec<_> = (0..4)
        .map(|i| {
            std::thread::spawn(move || {
                for _ in 0..=i {
                    COUNTER.with(|c| c.set(c.get() + 1));
                }
                COUNTER.with(Cell::get)
            })
        })
        .collect();
    let counts: Vec<u32> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    COUNTER.with(|c| c.set(c.get() + 100));

    let mut map = std::collections::HashMap::new();
    map.insert("random-state", 1);

    let since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    println!("fs: {text:?} ({} bytes, {entries} entries), {frameworks} frameworks", meta.len());
    println!("tls: threads {counts:?}, main {}", COUNTER.with(Cell::get));
    println!("time: {}s since epoch, {:?} elapsed", since_epoch.as_secs(), start.elapsed());
    println!("args: {:?}, HOME={:?}", std::env::args().collect::<Vec<_>>(), std::env::var("HOME").ok());
    std::panic::catch_unwind(|| panic!("caught")).unwrap_err();
    println!("unwind: ok");
}
