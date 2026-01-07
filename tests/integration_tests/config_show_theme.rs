use crate::common::wt_command;
use insta_cmd::assert_cmd_snapshot;

#[test]
fn test_show_theme() {
    let mut cmd = wt_command();
    cmd.arg("config").arg("shell").arg("show-theme");

    assert_cmd_snapshot!(cmd);
}
