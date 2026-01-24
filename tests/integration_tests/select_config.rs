use worktrunk::config::UserConfig;

#[test]
fn test_select_pager_config_deserialization() {
    // Verify that SelectConfig with pager field deserializes correctly
    let config_content = r#"
[select]
pager = "test-pager --custom-flag"
"#;

    let config: UserConfig = toml::from_str(config_content).unwrap();

    assert!(config.overrides.select.is_some());
    let select = config.overrides.select.unwrap();
    assert_eq!(select.pager, Some("test-pager --custom-flag".to_string()));
}

#[test]
fn test_select_pager_config_empty_string() {
    // Verify that empty string is valid TOML and deserializes
    let config_content = r#"
[select]
pager = ""
"#;

    let config: UserConfig = toml::from_str(config_content).unwrap();

    assert!(config.overrides.select.is_some());
    let select = config.overrides.select.unwrap();
    assert_eq!(select.pager, Some("".to_string()));
}

#[test]
fn test_select_config_optional() {
    // Verify that config without [select] section is still valid
    let config_content = r#"
[list]
full = true
"#;

    let config: UserConfig = toml::from_str(config_content).unwrap();
    assert!(config.overrides.select.is_none());
}
