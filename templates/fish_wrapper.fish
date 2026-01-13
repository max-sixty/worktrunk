# worktrunk shell integration for fish
# Sources full integration from binary on first use.
# Docs: https://worktrunk.dev/docs/shell-integration
# Check: {{ cmd }} config show | Uninstall: {{ cmd }} config shell uninstall

function {{ cmd }}
    command {{ cmd }} config shell init fish | source
    set -l wt_status $pipestatus[1]
    test $wt_status -eq 0; or return $wt_status
    {{ cmd }} $argv
end
