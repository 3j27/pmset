use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_pmset")
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("pmset-cli-{label}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn run_with_home(home: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env("HOME", home)
        .env_remove("SUDO_USER")
        .output()
        .unwrap()
}

#[test]
fn help_lists_the_actions_and_settings() {
    let output = Command::new(binary()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("displaysleepnow"));
    assert!(stdout.contains("displaysleep"));
    assert!(stdout.contains("lidwake"));
    assert!(stdout.contains("womp"));
}

#[test]
fn displaysleep_edits_the_omarchy_shell_config() {
    let home = temp_dir("displaysleep");
    let output = run_with_home(&home, &["displaysleep", "7"]);
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("7 minutes"));

    let config = home.join(".config/omarchy/shell.json");
    let text = fs::read_to_string(&config).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["idle"]["screensaver"], 420);

    let output = run_with_home(&home, &["-g"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("displaysleep   7"));

    let _ = fs::remove_dir_all(home);
}

#[test]
fn unsupported_settings_fail_clearly() {
    let output = Command::new(binary())
        .args(["disksleep", "5"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not supported"));
}

#[test]
fn read_only_get_commands_work() {
    let home = temp_dir("get");
    for option in ["-g", "-g cap", "-g batt", "-g ps", "-g assertions"] {
        let args: Vec<&str> = option.split_whitespace().collect();
        let output = run_with_home(&home, &args);
        assert!(output.status.success(), "{option}: {output:?}");
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn displaysleepnow_uses_hyprctl() {
    let home = temp_dir("hyprctl");
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let hyprctl = bin.join("hyprctl");
    fs::write(&hyprctl, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&hyprctl, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(binary())
        .arg("displaysleepnow")
        .env("HOME", &home)
        .env_remove("SUDO_USER")
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let _ = fs::remove_dir_all(home);
}
