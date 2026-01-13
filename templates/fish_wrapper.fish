# worktrunk shell integration for fish
# Sources full integration from binary on first use.
# Docs: https://worktrunk.dev/docs/shell-integration
# Check: {{ cmd }} config show | Uninstall: {{ cmd }} config shell uninstall

function {{ cmd }}
    command {{ cmd }} config shell init fish | source
    or return
    {{ cmd }} $argv
end
