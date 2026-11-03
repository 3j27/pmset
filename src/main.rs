mod actions;
mod logind;
mod omarchy;
mod power;
mod settings;
mod wol;

use std::env;
use std::process;

use anyhow::{bail, Result};

const HELP: &str = r#"pmset -- manipulate power management settings (Linux/Omarchy)

Usage:
  pmset [-a | -b | -c] [setting value] [...]
  pmset -g [option]
  pmset [displaysleepnow | sleepnow | lock | touch | noidle | boot]
  pmset -h | --help
  pmset -V | --version

Settings (value in minutes, or 0 to disable):
  displaysleep N   Blank the display after N idle minutes
                   (Omarchy shell idle screensaver timer)
  sleep N          Suspend the system after N idle minutes
                   (systemd-logind IdleActionSec)
  lidwake 0|1      Suspend when the lid is closed (0 disables)
  womp 0|1         Wake on LAN magic packet

Actions:
  displaysleepnow  Turn the display off immediately (Hyprland DPMS)
                   Wake it with 'omarchy system wake'
  sleepnow         Suspend the system immediately
  lock             Lock the session and turn the display off
  touch            Re-read settings (settings are already read live)
  noidle           Hold a sleep/idle inhibitor until interrupted
  boot             No-op; Linux tracks boot state itself

Getting:
  pmset -g              Show the settings currently in use
  pmset -g live         Same as -g
  pmset -g custom       Show settings for each power source (system-wide)
  pmset -g cap          Show the power management features supported here
  pmset -g batt         Show battery and power source state
  pmset -g ps           Show every power supply
  pmset -g ac           Show AC/USB supplies
  pmset -g assertions   Show systemd inhibitors that block sleep or idle
  pmset -g everything   Show all of the above

The -a, -b and -c flags are accepted for pmset compatibility. Linux settings are
system-wide, so all three apply to every power source. UPS-specific settings
(-u) are not supported.

Unsupported macOS settings include disksleep, hibernatemode, autorestart,
halfdim, lessbright, acwake, ring, sms, ttyskeepawake, destroyfvkeyonstandby,
and autopoweroff. Scheduled and repeating power events are also unsupported;
use systemd timers or rtcwake(8) instead.

Note: logind only applies an idle suspend when the session publishes an idle
hint. Omarchy's Hyprland session does not currently do this, so 'sleep' may not
fire there; use 'pmset sleepnow' for an immediate suspend.

Files:
  ~/.config/omarchy/shell.json
  /etc/systemd/logind.conf.d/99-pmset.conf
"#;

const USAGE: &str = "Usage: pmset [-a|-b|-c] [setting value] [...] | pmset -g [option] | pmset [displaysleepnow|sleepnow|lock|touch|noidle|boot]\nTry 'pmset --help' for details.\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    All,
    Battery,
    Charger,
    Ups,
}

#[derive(Debug, Default, PartialEq)]
struct Changes {
    displaysleep: Option<f64>,
    sleep: Option<f64>,
    lidwake: Option<bool>,
    womp: Option<bool>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.displaysleep.is_none()
            && self.sleep.is_none()
            && self.lidwake.is_none()
            && self.womp.is_none()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GetOption {
    Live,
    Custom,
    Cap,
    Batt,
    Ps,
    Ac,
    Assertions,
    Everything,
    Unsupported(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    DisplaySleepNow,
    SleepNow,
    Lock,
    Touch,
    NoIdle,
    Boot,
    Unsupported(String, Vec<String>),
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Help,
    Version,
    Get(GetOption),
    Set { source: Source, changes: Changes },
    Action(Action),
}

fn parse_args(args: &[String]) -> Result<Parsed> {
    let mut source = Source::All;
    let mut source_explicit = false;
    let mut get = false;
    let mut positionals = Vec::new();

    for arg in args {
        match arg.as_str() {
            "-a" => {
                source = Source::All;
                source_explicit = true;
            }
            "-b" => {
                source = Source::Battery;
                source_explicit = true;
            }
            "-c" => {
                source = Source::Charger;
                source_explicit = true;
            }
            "-u" => {
                source = Source::Ups;
                source_explicit = true;
            }
            "-g" => get = true,
            // Accepted for pmset compatibility; every setting here is applied
            // immediately, so -force has no further effect.
            "-force" | "--force" => {}
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            _ if arg.starts_with('-') => bail!("unknown option {arg}"),
            _ => positionals.push(arg.clone()),
        }
    }
    if get {
        if source_explicit {
            bail!("power source flags cannot be combined with -g");
        }
        let option = match positionals.as_slice() {
            [] => GetOption::Live,
            [option] => parse_get_option(option),
            _ => bail!("-g accepts at most one option"),
        };
        return Ok(Parsed::Get(option));
    }

    let Some(first) = positionals.first() else {
        print!("{USAGE}");
        bail!("no settings or command specified");
    };

    if let Some(action) = action_for(first, &positionals[1..])? {
        if source_explicit {
            bail!("power source flags are not valid with {first}");
        }
        return Ok(Parsed::Action(action));
    }

    if source == Source::Ups {
        bail!("UPS-specific settings are not supported on Linux");
    }
    if positionals.len() % 2 != 0 {
        bail!(
            "setting '{}' is missing a value",
            positionals.last().unwrap()
        );
    }

    let mut changes = Changes::default();
    for pair in positionals.chunks(2) {
        let key = pair[0].as_str();
        let value = pair[1].as_str();

        match key {
            "displaysleep" | "dim" => {
                changes.displaysleep = Some(parse_minutes(key, value)?);
            }
            "sleep" => {
                changes.sleep = Some(parse_minutes(key, value)?);
            }
            "lidwake" => {
                changes.lidwake = Some(parse_bool(key, value)?);
            }
            "womp" => {
                changes.womp = Some(parse_bool(key, value)?);
            }
            "disksleep" | "spindown" => bail!(
                "setting '{key}' is not supported: Linux disk spindown has no reliable equivalent \
                 across ATA and NVMe"
            ),
            "hibernatemode" | "hibernatefile" => bail!(
                "setting '{key}' is not supported: use 'omarchy hibernation setup' to configure \
                 hibernation"
            ),
            "schedule" | "repeat" | "relative" => unreachable!(),
            other => {
                bail!("setting '{other}' is not supported on Linux/Omarchy (see 'pmset --help')")
            }
        }
    }

    if changes.is_empty() {
        bail!("no settings specified");
    }

    Ok(Parsed::Set { source, changes })
}

fn action_for(first: &str, rest: &[String]) -> Result<Option<Action>> {
    let action = match first {
        "displaysleepnow" => Action::DisplaySleepNow,
        "sleepnow" => Action::SleepNow,
        "lock" => Action::Lock,
        "touch" => Action::Touch,
        "noidle" => Action::NoIdle,
        "schedule" | "repeat" | "relative" => {
            return Ok(Some(Action::Unsupported(first.to_string(), rest.to_vec())));
        }
        "boot" => Action::Boot,
        _ => return Ok(None),
    };

    if !rest.is_empty() {
        bail!("{first} does not take arguments");
    }
    Ok(Some(action))
}

fn parse_get_option(option: &str) -> GetOption {
    match option {
        "live" | "" => GetOption::Live,
        "custom" => GetOption::Custom,
        "cap" => GetOption::Cap,
        "batt" | "battery" => GetOption::Batt,
        "ps" | "power" => GetOption::Ps,
        "ac" | "adapter" => GetOption::Ac,
        "assertions" => GetOption::Assertions,
        "everything" => GetOption::Everything,
        other => GetOption::Unsupported(other.to_string()),
    }
}

fn parse_minutes(key: &str, value: &str) -> Result<f64> {
    let minutes = value.parse::<f64>().map_err(|_| {
        anyhow::anyhow!(
            "invalid value for {key}: '{value}' (expected a non-negative number of minutes)"
        )
    })?;
    if !minutes.is_finite() || minutes < 0.0 {
        bail!("invalid value for {key}: '{value}' (expected a non-negative number of minutes)");
    }
    if minutes > 0.0 && minutes < 1.0 / 60.0 {
        bail!("invalid value for {key}: '{value}' (minimum is one second)");
    }
    Ok(minutes)
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => bail!("invalid value for {key}: '{value}' (expected 0 or 1)"),
    }
}

fn get(option: GetOption) -> Result<()> {
    match option {
        GetOption::Live => settings::print_live(false),
        GetOption::Custom => settings::print_live(true),
        GetOption::Cap => power::print_capabilities(),
        GetOption::Batt => power::print_power(false),
        GetOption::Ps => power::print_power(true),
        GetOption::Ac => power::print_ac(),
        GetOption::Assertions => power::print_assertions(),
        GetOption::Everything => {
            settings::print_live(false)?;
            println!();
            power::print_capabilities()?;
            println!();
            power::print_power(false)?;
            println!();
            power::print_assertions()
        }
        GetOption::Unsupported(option) => match option.as_str() {
            "sched" => bail!(
                "scheduled power events are not supported on Linux; use systemd timers or rtcwake(8)"
            ),
            "ups" | "pslog" | "rawlog" => bail!(
                "pmset -g {option} is not supported on Linux; use UPower or NUT for UPS telemetry"
            ),
            _ => bail!(
                "pmset -g {option} is not supported on Linux/Omarchy (see 'pmset --help')"
            ),
        },
    }
}

fn run(args: &[String]) -> Result<()> {
    match parse_args(args)? {
        Parsed::Help => {
            print!("{HELP}");
            Ok(())
        }
        Parsed::Version => {
            println!("pmset {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Parsed::Get(option) => get(option),
        Parsed::Set { source, changes } => {
            if matches!(source, Source::Battery | Source::Charger) {
                eprintln!(
                    "pmset: note: Linux power settings are system-wide; applying to all power sources"
                );
            }
            settings::apply(changes)
        }
        Parsed::Action(action) => actions::run(action),
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(err) = run(&args) {
        eprintln!("pmset: {err:#}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_simple_settings() {
        let parsed = parse_args(&args(&["-a", "displaysleep", "10", "sleep", "30"])).unwrap();
        assert_eq!(
            parsed,
            Parsed::Set {
                source: Source::All,
                changes: Changes {
                    displaysleep: Some(10.0),
                    sleep: Some(30.0),
                    ..Default::default()
                },
            }
        );
    }

    #[test]
    fn parses_bool_settings_and_alias() {
        let parsed = parse_args(&args(&["dim", "5", "lidwake", "0", "womp", "1"])).unwrap();
        assert_eq!(
            parsed,
            Parsed::Set {
                source: Source::All,
                changes: Changes {
                    displaysleep: Some(5.0),
                    lidwake: Some(false),
                    womp: Some(true),
                    ..Default::default()
                },
            }
        );
    }

    #[test]
    fn parses_get_options() {
        assert_eq!(
            parse_args(&args(&["-g"])).unwrap(),
            Parsed::Get(GetOption::Live)
        );
        assert_eq!(
            parse_args(&args(&["-g", "batt"])).unwrap(),
            Parsed::Get(GetOption::Batt)
        );
        assert_eq!(
            parse_args(&args(&["-g", "everything"])).unwrap(),
            Parsed::Get(GetOption::Everything)
        );
    }

    #[test]
    fn parses_actions() {
        assert_eq!(
            parse_args(&args(&["displaysleepnow"])).unwrap(),
            Parsed::Action(Action::DisplaySleepNow)
        );
        assert_eq!(
            parse_args(&args(&["sleepnow"])).unwrap(),
            Parsed::Action(Action::SleepNow)
        );
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse_args(&args(&["lidwake", "2"])).is_err());
        assert!(parse_args(&args(&["sleep", "-1"])).is_err());
        assert!(parse_args(&args(&["sleep", "0.001"])).is_err());
        assert!(parse_args(&args(&["sleep"])).is_err());
        assert!(parse_args(&args(&["disksleep", "5"])).is_err());
        assert!(parse_args(&args(&["-g", "sched"])).is_ok());
    }

    #[test]
    fn rejects_source_with_action() {
        assert!(parse_args(&args(&["-a", "sleepnow"])).is_err());
    }
}
