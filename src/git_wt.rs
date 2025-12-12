// git-wt binary entry point - delegates to wt's main
// This exists to avoid Cargo's "file found in multiple build targets" warning
include!("main.rs");
