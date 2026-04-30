#![no_main]

use libfuzzer_sys::fuzz_target;
use worktrunk::git::GitRemoteUrl;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    if let Some(url) = GitRemoteUrl::parse(&input) {
        let _ = url.host();
        let _ = url.owner();
        let _ = url.repo();
        let _ = url.project_identifier();
        let _ = url.is_github();
        let _ = url.is_gitlab();
    }
});
