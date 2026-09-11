//! Deterministic seeds for the page-model mutation driver shared with the `page_model` fuzz target.

#[path = "support/page_edits.rs"]
mod page_edits;

#[test]
fn random_model_edits_publish_and_read_back() {
    for seed in [
        vec![0, 0, 3, 0, 5, b'a', b'b'],
        vec![1, 3, 2, 0, 0, 1, 2, 4, 1, 3, 5, 0, 2],
        vec![2, 2, 1, 3, 4, 0, 0, 7, 1, 0, 8, 9, 10],
        vec![3, 5, 4, 5, 0, 1, 7, 0, 30, 40, 6, 0, 0],
    ] {
        page_edits::run(&seed);
    }
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for _ in 0..400 {
        let mut input = Vec::new();
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let length = 8 + (state % 40) as usize;
        for _ in 0..length {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            input.push((state >> 24) as u8);
        }
        page_edits::run(&input);
    }
}
