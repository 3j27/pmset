use std::env;
use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub const DEFAULT_SCREENSAVER_SECONDS: u64 = 150;

#[derive(Debug)]
struct TargetUser {
    home: PathBuf,
    uid: u32,
    gid: u32,
    needs_chown: bool,
}

pub fn shell_config_path() -> Result<PathBuf> {
    Ok(target_user()?.home.join(".config/omarchy/shell.json"))
}

pub fn read_screensaver_seconds() -> u64 {
    if let Ok(path) = shell_config_path() {
        if let Some(seconds) = read_screensaver_from(&path) {
            return seconds;
        }
    }
    if let Some(path) = defaults_config_path() {
        if let Some(seconds) = read_screensaver_from(&path) {
            return seconds;
        }
    }
    DEFAULT_SCREENSAVER_SECONDS
}

fn read_screensaver_from(path: &Path) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    extract_screensaver(&value)
}

fn extract_screensaver(value: &Value) -> Option<u64> {
    let raw = value.get("idle")?.get("screensaver")?;
    raw.as_u64()
        .or_else(|| raw.as_f64().map(|number| number.max(0.0) as u64))
}

pub fn set_screensaver_seconds(seconds: u64) -> Result<()> {
    let target = target_user()?;
    let path = target.home.join(".config/omarchy/shell.json");

    let mut value = if path.is_file() {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if text.trim().is_empty() {
            // The shell treats an empty user config as absent.
            load_default_shell_config()
        } else {
            let parsed: Value = serde_json::from_str(&text).with_context(|| {
                format!(
                    "refusing to overwrite {} because it is not valid JSON",
                    path.display()
                )
            })?;
            if !parsed.is_object() {
                bail!(
                    "refusing to overwrite {} because it is not a JSON object",
                    path.display()
                );
            }
            parsed
        }
    } else {
        load_default_shell_config()
    };

    apply_screensaver(&mut value, seconds);
    atomic_write_json(&path, &value, &target)
}

fn apply_screensaver(value: &mut Value, seconds: u64) {
    let object = value.as_object_mut().expect("caller ensured an object");
    // The shell treats a user shell.json without version 1 as unreadable and
    // falls back to defaults. Match what the shell itself writes.
    object.insert("version".to_string(), json!(1));
    let idle = object
        .entry("idle".to_string())
        .or_insert_with(|| json!({}));
    if !idle.is_object() {
        *idle = json!({});
    }
    idle.as_object_mut()
        .expect("just ensured object")
        .insert("screensaver".to_string(), json!(seconds));
}

fn load_default_shell_config() -> Value {
    if let Some(path) = defaults_config_path() {
        if let Ok(text) = fs::read_to_string(path) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if value.is_object() {
                    return value;
                }
            }
        }
    }
    builtin_shell_config()
}

fn defaults_config_path() -> Option<PathBuf> {
    if let Some(omarchy_path) = env::var_os("OMARCHY_PATH") {
        let path = PathBuf::from(omarchy_path).join("config/omarchy/shell.json");
        if path.is_file() {
            return Some(path);
        }
    }
    for path in [
        "/usr/share/omarchy/config/omarchy/shell.json",
        "/usr/local/share/omarchy/config/omarchy/shell.json",
    ] {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Mirrors the fallback shell config bundled with omarchy-shell.
fn builtin_shell_config() -> Value {
    json!({
        "version": 1,
        "idle": {
            "screensaver": DEFAULT_SCREENSAVER_SECONDS,
            "lock": 300
        },
        "bar": {
            "position": "top",
            "background": "opaque",
            "layout": {
                "left": [
                    {"id": "omarchy.menu"},
                    {"id": "omarchy.workspaces"}
                ],
                "centerBegin": [],
                "center": [
                    {"id": "omarchy.clock", "format": "dddd HH:mm"}
                ],
                "centerEnd": [],
                "right": [
                    {"id": "omarchy.audio"}
                ]
            }
        },
        "plugins": []
    })
}

fn target_user() -> Result<TargetUser> {
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        if let Some(sudo_user) = env::var_os("SUDO_USER") {
            let name = sudo_user.to_string_lossy();
            if let Some(user) = user_from_passwd(&name)? {
                return Ok(TargetUser {
                    needs_chown: user.uid != 0,
                    ..user
                });
            }
            bail!("cannot find a home directory for sudo user '{name}'");
        }
    }

    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    Ok(TargetUser {
        home,
        uid: euid,
        gid: unsafe { libc::getegid() },
        needs_chown: false,
    })
}

fn user_from_passwd(name: &str) -> Result<Option<TargetUser>> {
    let text = fs::read_to_string("/etc/passwd")
        .context("failed to read /etc/passwd to resolve the invoking user")?;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 7 || fields[0] != name {
            continue;
        }
        let uid = fields[2]
            .parse()
            .with_context(|| format!("invalid uid for user '{name}'"))?;
        let gid = fields[3]
            .parse()
            .with_context(|| format!("invalid gid for user '{name}'"))?;
        return Ok(Some(TargetUser {
            home: PathBuf::from(fields[5]),
            uid,
            gid,
            needs_chown: false,
        }));
    }
    Ok(None)
}

fn atomic_write_json(path: &Path, value: &Value, target: &TargetUser) -> Result<()> {
    let parent = path
        .parent()
        .context("shell configuration path has no parent directory")?;
    create_dir_all_owned(parent, target)?;

    let mode = fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .unwrap_or(0o644);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("shell configuration path has no file name")?;
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));

    let contents = serde_json::to_string_pretty(value)
        .context("failed to serialize shell configuration")?
        + "\n";

    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", tmp.display()))?;
        if target.needs_chown {
            chown(&tmp, target.uid, target.gid)?;
        }
        fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn create_dir_all_owned(path: &Path, target: &TargetUser) -> Result<()> {
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        missing.push(current.to_path_buf());
        current = current
            .parent()
            .context("configuration path has no existing ancestor")?;
    }
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    if target.needs_chown {
        for directory in missing.iter().rev() {
            chown(directory, target.uid, target.gid)?;
        }
    }
    Ok(())
}

fn chown(path: &Path, uid: u32, gid: u32) -> Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())
        .context("configuration path contains a NUL byte")?;
    if unsafe { libc::chown(path.as_ptr(), uid, gid) } != 0 {
        bail!(
            "failed to change ownership of {}: {}",
            path.to_string_lossy(),
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_screensaver() {
        let value = json!({"idle": {"screensaver": 300, "lock": 600}});
        assert_eq!(extract_screensaver(&value), Some(300));
        let value = json!({"idle": {"screensaver": 300.5, "lock": 600}});
        assert_eq!(extract_screensaver(&value), Some(300));
        let value = json!({"idle": {"lock": 600}});
        assert_eq!(extract_screensaver(&value), None);
    }

    #[test]
    fn builtin_config_is_usable() {
        let value = builtin_shell_config();
        assert_eq!(
            extract_screensaver(&value),
            Some(DEFAULT_SCREENSAVER_SECONDS)
        );
        assert_eq!(value["version"], 1);
        assert!(value["idle"]["lock"].is_number());
    }

    #[test]
    fn updates_only_the_screensaver() {
        let mut value = json!({
            "version": 1,
            "idle": {"screensaver": 150, "lock": 300},
            "bar": {"position": "bottom"}
        });
        apply_screensaver(&mut value, 600);
        assert_eq!(value["idle"]["screensaver"], 600);
        assert_eq!(value["idle"]["lock"], 300);
        assert_eq!(value["bar"]["position"], "bottom");
    }

    #[test]
    fn creates_idle_object_when_missing() {
        let mut value = json!({"version": 1});
        apply_screensaver(&mut value, 60);
        assert_eq!(value["idle"]["screensaver"], 60);
    }
}
