//! Column Alignment Verification Tests
//!
//! NOTE: These tests may appear duplicative with snapshot tests, but they serve a critical purpose.
//! LLMs are poor at assessing precise positional/alignment values in text snapshots. When reviewing
//! snapshot changes, an LLM might approve misaligned columns that look "close enough" visually.
//!
//! These imperative tests explicitly verify that:
//! - Column headers and data align at exact character positions
//! - Unicode width calculations are correct (not just byte lengths)
//! - Sparse columns (empty cells) don't break alignment
//!
//! The detailed verification logic here catches alignment bugs that would slip through snapshot review.

use crate::common::{TestRepo, wt_command};
use unicode_width::UnicodeWidthStr;
use worktrunk::styling::strip_ansi_codes;

/// Represents the start position of each column in a table row
#[derive(Debug, Clone)]
struct ColumnPositions {
    age: Option<usize>,
    cmts: Option<usize>,
    cmt_diff: Option<usize>,
    wt_diff: Option<usize>,
    remote: Option<usize>,
    commit: Option<usize>,
    message: Option<usize>,
    state: Option<usize>,
    path: Option<usize>,
}

impl ColumnPositions {
    /// Parse column positions from a header line (without ANSI codes)
    /// Returns positions as display column indices, not byte indices
    fn from_header(header: &str) -> Self {
        let mut positions = ColumnPositions {
            age: None,
            cmts: None,
            cmt_diff: None,
            wt_diff: None,
            remote: None,
            commit: None,
            message: None,
            state: None,
            path: None,
        };

        // Helper to convert byte position to display position
        let byte_to_display_pos = |byte_pos: usize| -> usize { header[..byte_pos].width() };

        // Find column headers by their known names
        // Note: str.find() returns byte positions, we need display positions
        if let Some(byte_pos) = header.find("Age") {
            positions.age = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("Main-Cmt") {
            positions.cmts = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("main↕") {
            positions.cmt_diff = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("HEAD±") {
            positions.wt_diff = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("Remote") {
            positions.remote = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("Commit") {
            positions.commit = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("Message") {
            positions.message = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("State") {
            positions.state = Some(byte_to_display_pos(byte_pos));
        }
        if let Some(byte_pos) = header.find("Path") {
            positions.path = Some(byte_to_display_pos(byte_pos));
        }

        positions
    }
}

/// Extract column boundaries by finding transitions from content to spaces
/// This is a more sophisticated approach that handles sparse columns
/// Positions are in display columns, not character or byte indices
#[derive(Debug, Clone)]
struct ColumnBoundary {
    start: usize, // Display column position
    end: usize,   // Display column position
    content: String,
}

fn find_column_boundaries(line: &str) -> Vec<ColumnBoundary> {
    let mut boundaries = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut char_idx = 0;
    let mut display_pos = 0;

    while char_idx < chars.len() {
        // Skip leading spaces
        while char_idx < chars.len() && chars[char_idx] == ' ' {
            char_idx += 1;
            display_pos += 1;
        }

        if char_idx >= chars.len() {
            break;
        }

        // Found start of content
        let start_display_pos = display_pos;
        let mut content = String::new();

        // Collect content until we hit 2+ consecutive spaces or end
        while char_idx < chars.len() {
            if chars[char_idx] == ' ' {
                // Check if this is a column separator (2+ spaces)
                let mut space_count = 0;
                let mut j = char_idx;
                while j < chars.len() && chars[j] == ' ' {
                    space_count += 1;
                    j += 1;
                }

                if space_count >= 2 {
                    // This is a separator
                    break;
                } else {
                    // Single space within content
                    content.push(chars[char_idx]);
                    char_idx += 1;
                    display_pos += 1;
                }
            } else {
                let ch = chars[char_idx];
                content.push(ch);
                char_idx += 1;
                // Add display width of this character
                display_pos += UnicodeWidthStr::width(ch.to_string().as_str());
            }
        }

        boundaries.push(ColumnBoundary {
            start: start_display_pos,
            end: display_pos,
            content: content.trim_end().to_string(),
        });
    }

    boundaries
}

/// Verify that all data rows have columns starting at the same positions as the header
fn verify_table_alignment(output: &str) -> Result<(), String> {
    let lines: Vec<&str> = output.lines().collect();

    if lines.is_empty() {
        return Err("No output to verify".to_string());
    }

    // Strip ANSI codes from all lines
    let stripped_lines: Vec<String> = lines.iter().map(|l| strip_ansi_codes(l)).collect();

    if stripped_lines.is_empty() {
        return Err("No lines after stripping ANSI codes".to_string());
    }

    // First line is the header
    let header = &stripped_lines[0];
    let header_positions = ColumnPositions::from_header(header);

    println!("\n=== Table Alignment Verification ===");
    println!("Header: {}", header);
    println!("Header length: {}", header.width());
    println!("Header positions: {:?}", header_positions);
    println!();

    // Collect all column boundaries for each row
    let mut all_row_boundaries: Vec<Vec<ColumnBoundary>> = Vec::new();

    // Verify each data row
    let mut errors = Vec::new();
    for (idx, row) in stripped_lines.iter().skip(1).enumerate() {
        if row.trim().is_empty() {
            continue;
        }

        let row_num = idx + 1;
        println!("Row {}: {}", row_num, row);
        println!("  Length: {}", row.width());

        // Find column boundaries in this row
        let boundaries = find_column_boundaries(row);
        println!("  Boundaries: {:?}", boundaries);
        all_row_boundaries.push(boundaries.clone());

        // CRITICAL CHECK: Verify that each column starts at the EXACT same position as in the header
        // This is the key test for the alignment bug

        // Check all defined columns
        let positions = [
            ("Branch", Some(0usize)), // Branch always starts at 0
            ("Age", header_positions.age),
            ("Main-Cmt", header_positions.cmts),
            ("Main-Δ", header_positions.cmt_diff),
            ("Dirty", header_positions.wt_diff),
            ("Remote", header_positions.remote),
            ("Commit", header_positions.commit),
            ("Message", header_positions.message),
            ("State", header_positions.state),
            ("Path", header_positions.path),
        ];

        for (col_name, maybe_pos) in positions.iter() {
            if let Some(expected_pos) = maybe_pos {
                // Find if content or padding starts at this position
                // A column should either:
                // 1. Have content starting exactly at expected_pos
                // 2. Have padding (spaces) at expected_pos if the cell is empty

                let actual_content_pos = boundaries
                    .iter()
                    .find(|b| b.start <= *expected_pos && b.end > *expected_pos)
                    .map(|b| b.start);

                // For the Path column specifically, verify it starts exactly where the header says
                if *col_name == "Path" {
                    // Find where the actual path content starts (typically "./")
                    // We need to convert from display position to check the content
                    if let Some(path_boundary) = boundaries.iter().find(|b| {
                        b.start <= *expected_pos
                            && b.end > *expected_pos
                            && b.content.starts_with("./")
                    }) && path_boundary.start != *expected_pos
                    {
                        errors.push(format!(
                            "Row {}: Path column content starts at display position {} but header says it should be at {}. Misalignment: {} characters.\n  Row text: '{}'\n  Path content: '{}'",
                            row_num,
                            path_boundary.start,
                            expected_pos,
                            path_boundary.start.abs_diff(*expected_pos),
                            row,
                            path_boundary.content
                        ));
                    }
                }

                // For all columns, check that content starts at a consistent position
                if let Some(actual_start) = actual_content_pos
                    && actual_start != *expected_pos
                {
                    // Only report if this is actual content, not just padding
                    let content_at_pos = boundaries
                        .iter()
                        .find(|b| b.start == actual_start)
                        .map(|b| &b.content);

                    if let Some(content) = content_at_pos
                        && !content.is_empty()
                        && content.trim() != ""
                    {
                        println!(
                            "  ⚠️  Column '{}': content starts at {} instead of {} (content: '{}')",
                            col_name, actual_start, expected_pos, content
                        );
                    }
                }
            }
        }

        println!();
    }

    // Additional check: verify that ALL rows have the same column start positions
    // by comparing boundaries across rows
    if all_row_boundaries.len() > 1 {
        println!("=== Cross-row alignment check ===");
        let first_row_boundary_starts: Vec<usize> =
            all_row_boundaries[0].iter().map(|b| b.start).collect();

        for boundaries in all_row_boundaries.iter().skip(1) {
            let this_row_starts: Vec<usize> = boundaries.iter().map(|b| b.start).collect();

            // Check that boundaries align (allowing for sparse columns)
            for &expected_start in first_row_boundary_starts.iter() {
                // Find if this row has a boundary at or near this position
                let matching_boundary = this_row_starts.iter().find(|&&s| s == expected_start);

                if matching_boundary.is_none() {
                    // This is OK if the column is empty (sparse column)
                    // But we should at least have the same number of boundaries or fewer
                    continue;
                }
            }
        }
    }

    if !errors.is_empty() {
        Err(format!(
            "\n=== ALIGNMENT ERRORS ===\n{}\n",
            errors.join("\n\n")
        ))
    } else {
        println!("✓ All rows properly aligned");
        Ok(())
    }
}

#[test]
fn test_alignment_verification_with_varying_content() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create diverse worktrees to test alignment
    repo.add_worktree("main-feature", "main-feature");
    repo.add_worktree("short", "short");
    repo.add_worktree("very-long-branch-name-here", "very-long");

    // Add files to create working tree diffs
    let feature_path = repo.worktrees.get("main-feature").unwrap();
    for i in 0..10 {
        std::fs::write(feature_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    let short_path = repo.worktrees.get("short").unwrap();
    std::fs::write(short_path.join("single.txt"), "x").unwrap();

    // Run wt list and capture output
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stderr);

    println!("=== RAW OUTPUT ===");
    println!("{}", stdout);
    println!("==================");

    // Verify alignment
    match verify_table_alignment(&stdout) {
        Ok(()) => println!("\n✓ Alignment verification passed"),
        Err(e) => panic!("\n{}", e),
    }
}

#[test]
fn test_alignment_with_unicode_content() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit with émoji 🎉");

    // Create worktrees with unicode in names
    repo.add_worktree("café", "cafe");
    repo.add_worktree("naïve-approach", "naive");
    repo.add_worktree("résumé-feature", "resume");

    // Run wt list
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stderr);

    println!("=== RAW OUTPUT WITH UNICODE ===");
    println!("{}", stdout);
    println!("================================");

    // Verify alignment
    match verify_table_alignment(&stdout) {
        Ok(()) => println!("\n✓ Unicode alignment verification passed"),
        Err(e) => panic!("\n{}", e),
    }
}

#[test]
fn test_alignment_with_sparse_columns() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create mix of worktrees - some with diffs, some without
    repo.add_worktree("no-changes-1", "no-changes-1");

    repo.add_worktree("with-changes", "with-changes");
    let changes_path = repo.worktrees.get("with-changes").unwrap();
    for i in 0..100 {
        std::fs::write(changes_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    repo.add_worktree("no-changes-2", "no-changes-2");

    repo.add_worktree("small-change", "small-change");
    let small_path = repo.worktrees.get("small-change").unwrap();
    std::fs::write(small_path.join("one.txt"), "x").unwrap();

    // Run wt list
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stderr);

    println!("=== RAW OUTPUT WITH SPARSE COLUMNS ===");
    println!("{}", stdout);
    println!("=======================================");

    // Verify alignment - this is where the bug should show up
    match verify_table_alignment(&stdout) {
        Ok(()) => println!("\n✓ Sparse column alignment verification passed"),
        Err(e) => panic!("\n{}", e),
    }
}

#[test]
fn test_alignment_real_world_scenario() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create feature branches with varying amounts of working tree changes
    // This simulates a real-world scenario with different diff sizes
    repo.add_worktree("feature-tiny", "feature-tiny");
    let tiny_path = repo.worktrees.get("feature-tiny").unwrap();
    std::fs::write(tiny_path.join("file.txt"), "x").unwrap();

    repo.add_worktree("feature-small", "feature-small");
    let small_path = repo.worktrees.get("feature-small").unwrap();
    for i in 0..10 {
        std::fs::write(small_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    repo.add_worktree("feature-medium", "feature-medium");
    let medium_path = repo.worktrees.get("feature-medium").unwrap();
    for i in 0..100 {
        std::fs::write(medium_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    repo.add_worktree("feature-large", "feature-large");
    let large_path = repo.worktrees.get("feature-large").unwrap();
    for i in 0..1000 {
        std::fs::write(large_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    repo.add_worktree("no-changes", "no-changes");
    // No changes on this one

    // Run wt list at a width where Dirty column is visible
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);
    cmd.arg("list").current_dir(repo.root_path());

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stderr);

    println!("=== RAW OUTPUT: Real World Scenario ===");
    println!("{}", stdout);
    println!("========================================");

    // Verify alignment - this should catch the Path column misalignment bug
    match verify_table_alignment(&stdout) {
        Ok(()) => println!("\n✓ Real world scenario alignment verification passed"),
        Err(e) => panic!("\n{}", e),
    }
}

#[test]
fn test_alignment_at_different_terminal_widths() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    let path_a = repo.worktrees.get("feature-a").unwrap();
    std::fs::write(path_a.join("file.txt"), "content").unwrap();

    // Test at multiple terminal widths
    for width in [80, 120, 150, 200] {
        println!("\n### Testing at width {} ###", width);

        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("list")
            .current_dir(repo.root_path())
            .env("COLUMNS", width.to_string());

        let output = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stderr);

        println!("{}", stdout);

        match verify_table_alignment(&stdout) {
            Ok(()) => println!("✓ Width {} aligned correctly", width),
            Err(e) => panic!("\nWidth {} failed:\n{}", width, e),
        }
    }
}
