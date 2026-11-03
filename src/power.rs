use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::wol;

#[derive(Debug, Clone)]
struct Supply {
    name: String,
    kind: String,
    online: Option<bool>,
    status: Option<String>,
    capacity: Option<f64>,
    present: bool,
    energy_now: Option<f64>,
    energy_full: Option<f64>,
    power_now: Option<f64>,
    charge_now: Option<f64>,
    charge_full: Option<f64>,
    current_now: Option<f64>,
    voltage_now: Option<f64>,
    cycle_count: Option<u64>,
}

impl Supply {
    fn is_battery(&self) -> bool {
        self.kind.eq_ignore_ascii_case("battery")
    }

    fn is_ac(&self) -> bool {
        matches!(
            self.kind.to_ascii_lowercase().as_str(),
            "mains" | "usb" | "usb_c" | "ac" | "ups"
        )
    }

    fn percentage(&self) -> Option<f64> {
        if let Some(capacity) = self.capacity {
            return Some(capacity.clamp(0.0, 100.0));
        }
        if let (Some(now), Some(full)) = (self.energy_now, self.energy_full) {
            if full > 0.0 {
                return Some((now / full * 100.0).clamp(0.0, 100.0));
            }
        }
        if let (Some(now), Some(full)) = (self.charge_now, self.charge_full) {
            if full > 0.0 {
                return Some((now / full * 100.0).clamp(0.0, 100.0));
            }
        }
        None
    }

    fn minutes_remaining(&self) -> Option<u64> {
        let status = self.status.as_deref().unwrap_or_default();

        let (remaining, power) = if status.eq_ignore_ascii_case("discharging") {
            if let (Some(now), Some(power)) = (self.energy_now, self.power_now) {
                if power == 0.0 {
                    return None;
                }
                (now, power.abs())
            } else {
                let now = self.charge_now?;
                let current = self.current_now?.abs();
                if current <= 0.0 {
                    return None;
                }
                (now, current)
            }
        } else if status.eq_ignore_ascii_case("charging") {
            if let (Some(now), Some(full), Some(power)) =
                (self.energy_now, self.energy_full, self.power_now)
            {
                if power == 0.0 {
                    return None;
                }
                ((full - now).max(0.0), power.abs())
            } else {
                let now = self.charge_now?;
                let full = self.charge_full?;
                let current = self.current_now?.abs();
                if current <= 0.0 {
                    return None;
                }
                ((full - now).max(0.0), current)
            }
        } else if status.eq_ignore_ascii_case("full") || status.eq_ignore_ascii_case("not charging")
        {
            return Some(0);
        } else {
            return None;
        };

        let hours = remaining / power;
        if !hours.is_finite() || hours <= 0.0 {
            return Some(0);
        }
        Some((hours * 60.0).round() as u64)
    }
}

fn read_trim(directory: &Path, file: &str) -> Option<String> {
    fs::read_to_string(directory.join(file))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn read_number(directory: &Path, file: &str) -> Option<f64> {
    read_trim(directory, file)?.parse().ok()
}

fn read_uint(directory: &Path, file: &str) -> Option<u64> {
    read_trim(directory, file)?.parse().ok()
}

fn read_supplies() -> Vec<Supply> {
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();

    paths
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let kind = read_trim(&path, "type")?;
            let present = read_trim(&path, "present")
                .map(|value| value == "1")
                .unwrap_or(true);
            let supply = Supply {
                name,
                kind,
                online: read_trim(&path, "online").map(|value| value == "1"),
                status: read_trim(&path, "status"),
                capacity: read_number(&path, "capacity"),
                present,
                energy_now: read_number(&path, "energy_now"),
                energy_full: read_number(&path, "energy_full"),
                power_now: read_number(&path, "power_now"),
                charge_now: read_number(&path, "charge_now"),
                charge_full: read_number(&path, "charge_full"),
                current_now: read_number(&path, "current_now"),
                voltage_now: read_number(&path, "voltage_now"),
                cycle_count: read_uint(&path, "cycle_count"),
            };
            if supply.is_battery() {
                let has_data = supply.capacity.is_some()
                    || supply.energy_now.is_some()
                    || supply.charge_now.is_some()
                    || supply.status.is_some();
                has_data.then_some(supply)
            } else if supply.is_ac() {
                Some(supply)
            } else {
                None
            }
        })
        .collect()
}

fn source_name(supplies: &[Supply]) -> &'static str {
    if supplies
        .iter()
        .any(|supply| supply.is_ac() && supply.online == Some(true))
    {
        return "AC Power";
    }
    if supplies
        .iter()
        .any(|supply| supply.is_battery() && supply.present)
    {
        return "Battery Power";
    }
    "Unknown"
}

pub fn print_power(all: bool) -> Result<()> {
    let supplies = read_supplies();
    println!("Now drawing from '{}'", source_name(&supplies));

    let mut battery_number = 0;
    for supply in &supplies {
        if !all && !supply.is_battery() {
            continue;
        }
        if !all && !supply.present {
            continue;
        }

        if supply.is_battery() {
            print_battery(supply, battery_number);
            battery_number += 1;
        } else {
            println!(
                " -{} (id={}) online: {}",
                supply.name,
                supply.name,
                supply.online.unwrap_or(false)
            );
        }
    }

    if !all && battery_number == 0 {
        println!(" No batteries present.");
    } else if supplies.is_empty() {
        println!(" No power supplies found.");
    }
    Ok(())
}

fn print_battery(supply: &Supply, index: usize) {
    let percentage = supply
        .percentage()
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "unknown%".to_string());
    let status = match supply.status.as_deref().unwrap_or("unknown") {
        value if value.eq_ignore_ascii_case("full") => "charged".to_string(),
        value if value.eq_ignore_ascii_case("not charging") => {
            "AC attached; not charging".to_string()
        }
        value => value.to_ascii_lowercase(),
    };
    let remaining = supply
        .minutes_remaining()
        .map(|minutes| format!("{} remaining", format_duration(minutes)))
        .unwrap_or_else(|| "time remaining unknown".to_string());

    println!(
        " -InternalBattery-{index} (id={}) {percentage}; {status}; {remaining} present: {}",
        supply.name, supply.present
    );

    let cycles = supply
        .cycle_count
        .map(|cycles| format!("; cycles: {cycles}"))
        .unwrap_or_default();
    let voltage = supply
        .voltage_now
        .map(|voltage| format!("; {:.2} V", voltage / 1_000_000.0))
        .unwrap_or_default();
    if let (Some(now), Some(full)) = (supply.energy_now, supply.energy_full) {
        println!(
            "     capacity: {:.0} mWh / {:.0} mWh{cycles}{voltage}",
            now / 1000.0,
            full / 1000.0
        );
    } else if let (Some(now), Some(full)) = (supply.charge_now, supply.charge_full) {
        println!(
            "     capacity: {:.0} mAh / {:.0} mAh{cycles}{voltage}",
            now / 1000.0,
            full / 1000.0
        );
    }
}

fn format_duration(minutes: u64) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

pub fn print_ac() -> Result<()> {
    let supplies = read_supplies();
    let ac: Vec<&Supply> = supplies.iter().filter(|supply| supply.is_ac()).collect();
    if ac.is_empty() {
        println!("No AC or USB power supplies found.");
        return Ok(());
    }
    for supply in ac {
        println!(
            " -{} (id={}) online: {}",
            supply.name,
            supply.name,
            supply.online.unwrap_or(false)
        );
    }
    Ok(())
}

pub fn print_capabilities() -> Result<()> {
    let supplies = read_supplies();
    let battery = supplies
        .iter()
        .any(|supply| supply.is_battery() && supply.present);
    let ac = supplies.iter().any(Supply::is_ac);
    let backlights = backlights();
    let suspend = power_states()
        .iter()
        .any(|state| state == "mem" || state == "freeze");
    let hibernate = power_states().iter().any(|state| state == "disk");

    println!("System capabilities:");
    println!(" Battery: {}", yes_no(battery));
    println!(" AC adapter: {}", yes_no(ac));
    match backlights.as_slice() {
        [] => println!(" Display backlight: no"),
        [one] => println!(" Display backlight: yes ({one})"),
        many => println!(" Display backlight: yes ({})", many.join(", ")),
    }
    println!(" Lid switch: {}", yes_no(has_lid_switch()));
    println!(" Suspend: {}", yes_no(suspend));
    println!(" Hibernate: {}", yes_no(hibernate));
    println!(" Wake on LAN: {}", yes_no(wol::capable()));
    Ok(())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn backlights() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/sys/class/backlight") else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn power_states() -> Vec<String> {
    fs::read_to_string("/sys/power/state")
        .map(|text| text.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn has_lid_switch() -> bool {
    if Path::new("/proc/acpi/button/lid").is_dir() {
        return true;
    }
    let Ok(entries) = fs::read_dir("/sys/class/input") else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = fs::read_to_string(path.join("name"))
            .or_else(|_| fs::read_to_string(path.join("device/name")));
        if let Ok(name) = name {
            if name.to_ascii_lowercase().contains("lid") {
                return true;
            }
        }
    }
    false
}

pub fn print_assertions() -> Result<()> {
    let output = Command::new("systemd-inhibit")
        .args(["--list", "--no-pager"])
        .output()
        .context("could not run systemd-inhibit")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "systemd-inhibit failed with {}: {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        println!("No power assertions.");
    } else {
        println!("Power assertions (systemd inhibitors):");
        print!("{stdout}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(59), "0:59");
        assert_eq!(format_duration(90), "1:30");
        assert_eq!(format_duration(600), "10:00");
    }

    #[test]
    fn computes_percentage_from_energy() {
        let supply = Supply {
            name: "BAT0".into(),
            kind: "Battery".into(),
            online: None,
            status: Some("Discharging".into()),
            capacity: None,
            present: true,
            energy_now: Some(25.0),
            energy_full: Some(100.0),
            power_now: Some(-10.0),
            charge_now: None,
            charge_full: None,
            current_now: None,
            voltage_now: None,
            cycle_count: None,
        };
        assert_eq!(supply.percentage(), Some(25.0));
        assert_eq!(supply.minutes_remaining(), Some(150));
    }

    #[test]
    fn computes_discharge_time_from_current() {
        let supply = Supply {
            name: "BAT0".into(),
            kind: "Battery".into(),
            online: None,
            status: Some("Discharging".into()),
            capacity: Some(50.0),
            present: true,
            energy_now: None,
            energy_full: None,
            power_now: None,
            charge_now: Some(2_000_000.0),
            charge_full: Some(4_000_000.0),
            current_now: Some(1_000_000.0),
            voltage_now: Some(12_000_000.0),
            cycle_count: None,
        };
        // 2 Ah at 1 A => 2 hours.
        assert_eq!(supply.minutes_remaining(), Some(120));
    }
}
