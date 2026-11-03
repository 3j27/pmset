use anyhow::{bail, Result};

use crate::{logind, omarchy, wol, Changes};

pub fn print_live(custom: bool) -> Result<()> {
    let logind = logind::read_effective();
    let displaysleep = omarchy::read_screensaver_seconds() as f64 / 60.0;
    let sleep = logind.sleep_minutes();
    let lidwake = logind.lidwake();
    let womp = wol::enabled();

    if custom {
        println!(
            "Linux power settings are system-wide; per-power-source settings are not available."
        );
        println!();
    }

    println!("System-wide power settings:");
    println!("Currently in use:");
    setting("displaysleep", &format_minutes(displaysleep));
    setting("sleep", &format_minutes(sleep));
    setting("lidwake", if lidwake { "1" } else { "0" });
    setting("womp", if womp { "1" } else { "0" });
    Ok(())
}

fn setting(name: &str, value: &str) {
    println!(" {name:<14} {value}");
}

fn format_minutes(minutes: f64) -> String {
    if minutes <= 0.0 {
        return "0".to_string();
    }
    // Four decimals represent one second precisely enough for display while
    // still printing whole minutes without a decimal point.
    let text = format!("{minutes:.4}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn apply(changes: Changes) -> Result<()> {
    if (changes.sleep.is_some() || changes.lidwake.is_some() || changes.womp.is_some())
        && unsafe { libc::geteuid() } != 0
    {
        bail!("changing sleep, lidwake, or womp requires root (try sudo)");
    }

    let mut errors = Vec::new();

    if let Some(minutes) = changes.displaysleep {
        let seconds = (minutes * 60.0).round();
        let result = if seconds > u64::MAX as f64 {
            Err(anyhow::anyhow!("displaysleep value is too large"))
        } else {
            omarchy::set_screensaver_seconds(seconds as u64)
        };
        match result {
            Ok(()) => {
                if minutes == 0.0 {
                    println!("Setting displaysleep to 0 (never)");
                } else {
                    println!(
                        "Setting displaysleep to {} minutes",
                        format_minutes(minutes)
                    );
                }
            }
            Err(err) => errors.push(format!("displaysleep: {err:#}")),
        }
    }

    if changes.sleep.is_some() || changes.lidwake.is_some() {
        let update = logind::Update {
            sleep_minutes: changes.sleep,
            lidwake: changes.lidwake,
        };
        match logind::apply(&update) {
            Ok(()) => {
                if let Some(minutes) = changes.sleep {
                    if minutes == 0.0 {
                        println!("Setting sleep to 0 (never)");
                    } else {
                        println!("Setting sleep to {} minutes", format_minutes(minutes));
                    }
                }
                if let Some(lidwake) = changes.lidwake {
                    println!("Setting lidwake to {}", if lidwake { 1 } else { 0 });
                }
            }
            Err(err) => errors.push(format!("logind: {err:#}")),
        }
    }

    if let Some(enabled) = changes.womp {
        match wol::set_enabled(enabled) {
            Ok(()) => println!("Setting womp to {}", if enabled { 1 } else { 0 }),
            Err(err) => errors.push(format!("womp: {err:#}")),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        bail!("{}", errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_minutes() {
        assert_eq!(format_minutes(0.0), "0");
        assert_eq!(format_minutes(5.0), "5");
        assert_eq!(format_minutes(2.5), "2.5");
        assert_eq!(format_minutes(1.0 / 60.0), "0.0167");
        assert_eq!(format_minutes(0.02), "0.02");
    }
}
