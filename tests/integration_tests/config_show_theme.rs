use crate::common::{add_standard_env_redactions, wt_command};
use insta_cmd::assert_cmd_snapshot;

#[test]
fn test_show_theme() {
    let mut settings = insta::Settings::clone_current();
    add_standard_env_redactions(&mut settings);
    settings.bind(|| {
        let mut cmd = wt_command();
        cmd.arg("config").arg("shell").arg("show-theme");

        assert_cmd_snapshot!(cmd);
    });
}
