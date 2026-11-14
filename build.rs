use std::error::Error;
use vergen_gitcl::{Emitter, Gitcl};

pub fn main() -> Result<(), Box<dyn Error>> {
    // Describe from release tags only. Non-version tags (such as the
    // `release-recovery-*` tags the release workflow dispatches from) would
    // otherwise become the reported version.
    let git = Gitcl::builder()
        .describe(true, true, Some("v[0-9]*"))
        .build();
    Emitter::default().add_instructions(&git)?.emit()?;
    Ok(())
}
