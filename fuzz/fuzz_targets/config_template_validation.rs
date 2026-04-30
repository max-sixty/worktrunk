#![no_main]

use libfuzzer_sys::fuzz_target;
use worktrunk::config::{template_references_var, validate_template_syntax};

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let (template, var) = input.split_once('\n').unwrap_or((input.as_ref(), "branch"));
    let var = var.trim();
    let var = if var.is_empty() { "branch" } else { var };

    let _ = validate_template_syntax(template, "fuzz-template");
    let _ = template_references_var(template, var);
});
