#![no_main]

use libfuzzer_sys::fuzz_target;
use worktrunk::config::migrate_content;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = migrate_content(&input);
});
