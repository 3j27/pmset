use std::io;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::Action;

/// Hyprland wakes a DPMS-blanked display on a key press. The key that launched
/// this command can still be in the compositor's input queue, so install a
/// short-lived guard that suppresses key-press wakeups until that key is
/// released. The guard is event-driven: it does not delay blanking.
const KEY_WAKE_GUARD: &str = r#"
local g = _G.__pmset_dpms_guard
if g == nil then
  g = {
    original = hl.get_config("misc.key_press_enables_dpms"),
    generation = 0,
    sub = nil,
    timer = nil,
    done = false
  }
  _G.__pmset_dpms_guard = g
else
  if g.sub then g.sub:remove(); g.sub = nil end
  if g.timer then g.timer:set_enabled(false); g.timer = nil end
  g.done = false
end
g.generation = g.generation + 1
local my_gen = g.generation

local function cleanup()
  if g.done or g.generation ~= my_gen then return end
  g.done = true
  if g.sub then g.sub:remove(); g.sub = nil end
  if g.timer then g.timer:set_enabled(false); g.timer = nil end
  hl.config({ misc = { key_press_enables_dpms = g.original } })
  _G.__pmset_dpms_guard = nil
end

g.sub = hl.on("input.keyboard.key", function(keycode, time, state)
  if state == 0 then
    hl.timer(cleanup, { timeout = 1, type = "oneshot" })
  end
end)
g.timer = hl.timer(cleanup, { timeout = 1000, type = "oneshot" })
hl.config({ misc = { key_press_enables_dpms = false } })
return "ok"
"#;

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
    install_key_wake_guard();
    dispatch_dpms_off()
}

fn install_key_wake_guard() {
    match Command::new("hyprctl")
        .args(["eval", KEY_WAKE_GUARD])
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(_) => eprintln!(
            "pmset: warning: could not install the Hyprland key-wake guard; the display may wake immediately"
        ),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => eprintln!(
            "pmset: warning: could not install the Hyprland key-wake guard ({err}); the display may wake immediately"
        ),
    }
}

fn dispatch_dpms_off() -> Result<()> {
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
