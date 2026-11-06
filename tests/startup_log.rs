use std::path::Path;
use std::process::Command;

#[test]
fn rejected_overrides_remain_logged_after_framebuffer_startup() {
    let root = std::env::temp_dir().join(format!("degauss-startup-log-{}", std::process::id()));
    std::fs::create_dir_all(root.join("games/Test")).unwrap();
    std::fs::create_dir_all(root.join("menu/_Computer")).unwrap();
    std::fs::write(root.join("menu/_Computer/Test.rbf"), []).unwrap();
    let mut config: toml::Value = toml::from_str(include_str!("../degauss.toml")).unwrap();
    config["game_roots"] = toml::Value::Array(vec![toml::Value::String(
        root.join("games").to_str().unwrap().to_string(),
    )]);
    config["menu_root"] = root.join("menu").to_str().unwrap().into();
    std::fs::write(root.join("degauss.toml"), toml::to_string(&config).unwrap()).unwrap();
    std::fs::write(
        root.join("systems.toml"),
        "[[systems]]\nname = 'Test'\nid = 'Test'\nfolders = ['Test']\nrbf = '_Computer/Test'\nextensions = ['bin']\n",
    )
    .unwrap();
    let user = "[library_folders]\nStartupWarningRegression = ['invalid']\n";
    let user_path = root.join("degauss-user.toml");
    std::fs::write(&user_path, user).unwrap();
    let device = root.join("not-a-framebuffer");
    std::fs::write(&device, []).unwrap();
    let log_path = Path::new("/tmp/degauss.log");
    struct RestoreLog(Option<Vec<u8>>);
    impl Drop for RestoreLog {
        fn drop(&mut self) {
            match &self.0 {
                Some(contents) => std::fs::write("/tmp/degauss.log", contents)
                    .expect("restore the original frontend log"),
                None => {
                    if let Err(error) = std::fs::remove_file("/tmp/degauss.log") {
                        assert_eq!(
                            error.kind(),
                            std::io::ErrorKind::NotFound,
                            "restore the original absent log: {error}"
                        );
                    }
                }
            }
        }
    }
    let _restore_log = RestoreLog(match std::fs::read(log_path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("save the original frontend log: {error}"),
    });
    let previous_run = "previous frontend run\n";
    std::fs::write(log_path, previous_run).unwrap();

    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_degauss"));
        command
            .env_remove("DEGAUSS_MENU_OWNER")
            .arg("--config")
            .arg(root.join("degauss.toml"))
            .arg("--systems")
            .arg(root.join("systems.toml"))
            .args(["--system", "Test", "--device"])
            .arg(&device);
        command
    };
    // A real ordinary file can be locked, but the actual framebuffer ioctl
    // rejects it. Startup diagnostics must survive that later failure.
    let result = command().output().unwrap();
    assert!(!result.status.success());
    #[cfg(target_os = "linux")]
    assert!(String::from_utf8_lossy(&result.stderr).contains("not-a-framebuffer"));
    #[cfg(not(target_os = "linux"))]
    assert!(String::from_utf8_lossy(&result.stderr).contains("this build has no framebuffer"));
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(log.contains("ignoring"));
    assert!(log.contains(user_path.to_str().unwrap()));
    assert!(log.contains("StartupWarningRegression"));
    assert_eq!(log.contains(previous_run), !cfg!(target_os = "linux"));
    assert_eq!(std::fs::read_to_string(&user_path).unwrap(), user);

    // Diagnostics and a launcher whose owning Main has gone away must not
    // clear another frontend session's log or acquire its framebuffer.
    std::fs::write(log_path, previous_run).unwrap();
    assert!(command().arg("--version").status().unwrap().success());
    #[cfg(target_os = "linux")]
    {
        assert!(!Path::new("/proc/2147483647").exists());
        assert!(command()
            .env("DEGAUSS_MENU_OWNER", "2147483647:1")
            .status()
            .unwrap()
            .success());
    }
    assert_eq!(std::fs::read_to_string(log_path).unwrap(), previous_run);
    std::fs::remove_dir_all(root).unwrap();
}
