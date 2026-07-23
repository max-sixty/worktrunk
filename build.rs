use std::error::Error;
use vergen_gitcl::{Emitter, Gitcl};

pub fn main() -> Result<(), Box<dyn Error>> {
    let git = Gitcl::builder().describe(true, true, None).build();
    Emitter::default().add_instructions(&git)?.emit()?;
    Ok(())
}
