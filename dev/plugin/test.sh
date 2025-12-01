#!/bin/bash
# Manual testing guide for the wt-bridge plugin.

if [ -n "$ZELLIJ" ] && [[ "$ZELLIJ_SESSION_NAME" == wt:* ]]; then
    cat <<'EOF'
Inside workspace. Test the plugin:

    zellij pipe --name wt -- 'select|/tmp'

Expected:
  - First run:  New pane opens in /tmp
  - Again:      Existing pane focuses

If nothing happens, exit (Ctrl+O D) and re-run 'wt ui'.
EOF
else
    cat <<'EOF'
Test the plugin:

1. wt ui                                    # Enter workspace
2. Grant permissions when prompted
3. zellij pipe --name wt -- 'select|/tmp'   # Test pipe message

Expected: New pane opens in /tmp.
EOF
fi
