use ssh_files::Route;

#[test]
fn selected_hostname_keeps_alias_options_in_actual_openssh_config_resolution() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::write(&config, "Host fixture-alias\n HostName old-address.invalid\n User fixture-user\n Port 22222\n IdentityAgent none\n IdentityFile fixture-key\n ProxyCommand fixture-proxy %h %p\n").unwrap();
    let route = Route {
        host: "fixture-alias".into(),
        hostname: Some("127.0.0.1".into()),
        config: Some(config),
        user: None,
        port: None,
    };
    let source = route.command().unwrap();
    let output = std::process::Command::new(source.get_program())
        .arg("-G")
        .args(source.get_args())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "host fixture-alias",
        "hostname 127.0.0.1",
        "user fixture-user",
        "port 22222",
        "identityfile fixture-key",
        "proxycommand fixture-proxy %h %p",
    ] {
        assert!(
            config.lines().any(|line| line == expected),
            "Missing {expected:?}: {config}"
        );
    }
    let mut unchanged = route.clone();
    unchanged.hostname = None;
    let source = unchanged.command().unwrap();
    let output = std::process::Command::new(source.get_program())
        .arg("-G")
        .args(source.get_args())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|line| line == "hostname old-address.invalid")
    );
}

#[test]
fn hostname_override_rejects_option_and_control_injection() {
    for hostname in [
        "",
        "-oProxyCommand=bad",
        "host\nProxyCommand=bad",
        "two hosts",
    ] {
        let route = Route {
            host: "alias".into(),
            hostname: Some(hostname.into()),
            config: None,
            user: None,
            port: None,
        };
        assert!(route.command().is_err());
    }
}
