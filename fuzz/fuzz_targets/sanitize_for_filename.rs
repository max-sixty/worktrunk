#![no_main]

use libfuzzer_sys::fuzz_target;
use worktrunk::path::sanitize_for_filename;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = sanitize_for_filename(&input);
});
