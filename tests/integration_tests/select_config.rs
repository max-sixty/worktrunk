use worktrunk::config::UserConfig;
use worktrunk::config::migrate_content;

#[test]
fn test_select_pager_config_migrated_to_switch_picker() {
    // [select] is migrated to [switch.picker] at the TOML level before parsing
    let content = r#"
[select]
pager = "test-pager --custom-flag"
"#;
    let migrated = migrate_content(content);
    let config: UserConfig = toml::from_str(&migrated).unwrap();
    let picker = config.switch_picker(None);
    assert_eq!(picker.pager.as_deref(), Some("test-pager --custom-flag"));
}

#[test]
fn test_select_config_optional() {
    // Config without [select] section is still valid
    let content = r#"
[list]
full = true
"#;
    let config: UserConfig = toml::from_str(content).unwrap();
    assert_eq!(config.switch, worktrunk::config::SwitchConfig::default());
}
