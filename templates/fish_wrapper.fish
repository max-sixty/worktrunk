# worktrunk shell integration for fish
# Sources full integration from binary on first use.
# Docs: https://worktrunk.dev/docs/shell-integration
# Check: {{ cmd }} config show | Uninstall: {{ cmd }} config shell uninstall

function {{ cmd }}
    command {{ cmd }} config shell init fish | source
    # Check both command exit code ($pipestatus[1]) and source exit code ($pipestatus[2])
    # If source fails, the function isn't replaced and we'd infinite-loop calling ourselves
    set -l wt_status $pipestatus[1]
    set -l source_status $pipestatus[2]
    test $wt_status -eq 0; or return $wt_status
    test $source_status -eq 0; or return $source_status
    {{ cmd }} $argv
end
