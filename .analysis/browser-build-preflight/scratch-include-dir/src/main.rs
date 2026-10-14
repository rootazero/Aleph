use include_dir::{include_dir, Dir};
static EMPTY: Dir = include_dir!("$CARGO_MANIFEST_DIR/emptydir");
static FILLED: Dir = include_dir!("$CARGO_MANIFEST_DIR/filled");
fn main() { println!("empty={} filled={}", EMPTY.entries().len(), FILLED.entries().len()); }
