use std::io;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::Action;

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::DisplaySleepNow => display_sleep_now(),
        Action::SleepNow => sleep_now(),
        Action::Lock => lock_session(),
        Action::Touch => {
            println!("Settings are read directly from the system; nothing to reload.");
            Ok(())
        }
        Action::NoIdle => no_idle(),
        Action::Boot => {
            println!("pmset boot is not needed on Linux; the kernel tracks boot state itself.");
            Ok(())
        }
        Action::Unsupported(name, rest) => unsupported(&name, &rest),
    }
}

fn display_sleep_now() -> Result<()> {
    // Hyprland 0.5x accepts Lua dispatch expressions, while older releases use
    // the keyword form. Try both and keep the last useful error.
    let attempts: [&[&str]; 2] = [
        &["dispatch", "hl.dsp.dpms({ action = \"disable\" })"],
        &["dispatch", "dpms", "off"],
    ];

    let mut last_error = String::from("hyprctl is not running");
    for args in attempts {
        match Command::new("hyprctl").args(args).output() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let detail = [stderr.trim(), stdout.trim()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                last_error = format!("hyprctl {} failed: {detail}", args.join(" "));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                last_error = "hyprctl was not found in PATH".to_string();
            }
            Err(err) => {
                last_error = format!("could not run hyprctl: {err}");
            }
        }
    }

    bail!("could not turn off the display ({last_error})")
}

fn sleep_now() -> Result<()> {
    match run_checked(Command::new("loginctl").arg("suspend"), "loginctl suspend") {
        Ok(()) => Ok(()),
        Err(loginctl_error) => {
            if !command_missing(&loginctl_error) {
                eprintln!("pmset: warning: {loginctl_error:#}; trying systemctl");
            }
            match run_checked(
                Command::new("systemctl").arg("suspend"),
                "systemctl suspend",
            ) {
                Ok(()) => Ok(()),
                Err(systemctl_error) => {
                    bail!("could not suspend: {loginctl_error:#}; {systemctl_error:#}")
                }
            }
        }
    }
}

fn lock_session() -> Result<()> {
    match Command::new("omarchy").args(["system", "lock"]).status() {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => {
            eprintln!(
                "pmset: warning: 'omarchy system lock' exited with {status}; falling back to loginctl"
            );
        }
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            eprintln!("pmset: warning: could not run 'omarchy system lock' ({err}); falling back to loginctl");
        }
        Err(_) => {}
    }

    run_checked(
        Command::new("loginctl").arg("lock-session"),
        "loginctl lock-session",
    )
}

fn no_idle() -> Result<()> {
    let status = Command::new("systemd-inhibit")
        .args([
            "--what=idle:sleep",
            "--who=pmset",
            "--why=pmset noidle",
            "--mode=block",
            "sleep",
            "infinity",
        ])
        .status()
        .context("could not run systemd-inhibit")?;

    // Ctrl-C terminates the child by signal, which is the expected way to stop.
    if status.success() || status.code().is_none() {
        Ok(())
    } else {
        bail!("systemd-inhibit exited with {status}");
    }
}

fn unsupported(name: &str, rest: &[String]) -> Result<()> {
    let extra = if rest.is_empty() {
        String::new()
    } else {
        format!(" (got: {})", rest.join(" "))
    };
    match name {
        "schedule" | "repeat" => bail!(
            "scheduled and repeating power events are not supported on Linux; use systemd timers \
             or rtcwake(8){extra}"
        ),
        "relative" => {
            bail!("relative wake is not supported on Linux; use 'rtcwake -s SECONDS'{extra}")
        }
        other => bail!("command '{other}' is not supported on Linux/Omarchy{extra}"),
    }
}

fn run_checked(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("could not run {description}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{description} failed with {status}");
    }
}

/// `run_checked` wraps the io::Error, so look through the chain for NotFound.
fn command_missing(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .any(|err| err.kind() == io::ErrorKind::NotFound)
}
