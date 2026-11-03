# pmset for Linux and Omarchy

A small Rust workalike for macOS [`pmset(1)`](https://www.manpagez.com/man/1/pmset/),
implementing the parts of `pmset` that have a clear Linux/Omarchy equivalent.
It talks to the kernel and system services directly; it is not a shell-script
wrapper around macOS commands.

```
Usage:
  pmset [-a | -b | -c] [setting value] [...]
  pmset -g [option]
  pmset [displaysleepnow | sleepnow | lock | touch | noidle | boot]
```

## Supported commands

| macOS `pmset` | Linux/Omarchy implementation |
| --- | --- |
| `displaysleep N` | Sets `idle.screensaver` in `~/.config/omarchy/shell.json` (minutes, `0` disables) |
| `sleep N` | Writes `IdleAction`/`IdleActionSec` to `/etc/systemd/logind.conf.d/99-pmset.conf` |
| `lidwake 0\|1` | Writes `HandleLidSwitch` and `HandleLidSwitchExternalPower` to the same logind drop-in |
| `womp 0\|1` | Sets `WAKE_MAGIC` through the kernel `SIOCETHTOOL` Wake-on-LAN ioctl |
| `displaysleepnow` | Turns the display off with Hyprland DPMS (compositor keeps running) |
| `sleepnow` | `loginctl suspend`, falling back to `systemctl suspend` |
| `lock` | `omarchy system lock`, falling back to `loginctl lock-session` |
| `noidle` | Holds a `systemd-inhibit --what=idle:sleep` block inhibitor until interrupted |
| `touch` | No-op; settings are read from the live system |
| `boot` | No-op; Linux does not need a boot-complete notification |
| `-g`, `-g live`, `-g custom` | Show effective settings |
| `-g cap` | Show detected power capabilities |
| `-g batt` / `-g ps` | Read `/sys/class/power_supply` |
| `-g ac` / `-g adapter` | Show AC/USB supplies |
| `-g assertions` | Show systemd inhibitors via `systemd-inhibit --list` |
| `-g everything` | All of the above |

`-a`, `-b`, and `-c` are accepted for command-line compatibility. On Linux the
underlying settings are system-wide, so all three apply to every power source.
`-u` (UPS emergency thresholds) is rejected with an explanation.

Values are in minutes and may be fractional. Omarchy stores display timers in
seconds, so `pmset displaysleep 2.5` and the built-in 150-second default both
round-trip exactly.

## Build and install

```sh
cargo build --release
sudo make install          # /usr/local/bin/pmset + man page
```

Or install just the binary:

```sh
cargo install --path .
```

## Examples

Show the current settings and the battery:

```sh
pmset -g
pmset -g batt
```

Sleep the display immediately (DPMS off, session stays unlocked):

```sh
pmset displaysleepnow
```

Blank the display after 5 idle minutes and configure an idle suspend after 30:

```sh
pmset -a displaysleep 5 sleep 30
```

See the idle-suspend caveat below: on Omarchy, use `pmset sleepnow` for an
immediate suspend.

Disable idle suspend and lid suspend:

```sh
sudo pmset sleep 0 lidwake 0
```

Enable Wake-on-LAN magic packets:

```sh
sudo pmset womp 1
```

## How `displaysleep` maps

Omarchy's shell owns the idle timers. `pmset displaysleep N` updates
`idle.screensaver` in the user's `~/.config/omarchy/shell.json`, preserving the
rest of the file. The shell watches this file, so no restart is needed.
`idle.lock` is left alone, which means the lock screen keeps its own timeout.

The file is user-owned, so `pmset displaysleep` does **not** require root when
run as the desktop user. When run through `sudo`, `pmset` follows `SUDO_USER`
and edits that user's config rather than root's.

## How `sleep` maps

`pmset sleep N` writes `IdleAction=suspend` and `IdleActionSec=Nmin` to the
logind drop-in, which is systemd's standard idle-suspend mechanism.  logind
only applies an idle action when the session publishes an idle hint.  Omarchy's
Hyprland/uwsm session does not currently set one, so on Omarchy this timer may
not fire; use `pmset sleepnow` for an immediate suspend, or a compositor idle
daemon such as `hypridle` if you need an automatic idle suspend.  The setting
works on desktops that do publish idle hints.

## How `displaysleepnow` maps

`pmset displaysleepnow` asks Hyprland to disable DPMS:

1. `hyprctl dispatch 'hl.dsp.dpms({ action = "disable" })'` (Hyprland 0.5x+)
2. `hyprctl dispatch dpms off` (older Hyprland)

Hyprland wakes a DPMS-blanked display on a key press
(`misc:key_press_enables_dpms`). The Enter key that starts `pmset` can still be
in the compositor's input queue, which would turn the display straight back on.

`displaysleepnow` installs a short-lived Hyprland Lua guard before blanking:
it disables key-press wakeups, waits for the launching key to be released,
then restores the original setting. The guard is event-driven and clears
itself after one second if no key event arrives, so there is no delay before
the display turns off. If the guard cannot be installed, `pmset` still blanks
the display normally and prints a warning.

This is the equivalent of Omarchy's `omarchy brightness display off`: the panel
and compositor outputs turn off, and Omarchy's idle/wake handling can turn them
back on. If you turn the display off manually, restore it with
`omarchy system wake` (or `omarchy brightness display on`). It does not write
the backlight sysfs value to zero, which would leave the panel dark until
brightness was explicitly restored. If you want that MacBook-style behavior
instead, use `brightnessctl -d apple-panel-bl set 0` directly.

## Privileges

- `displaysleep` edits a user config file and needs no root.
- `sleep`, `lidwake`, and `womp` change system state and require root.
- `sleepnow`, `lock`, and `displaysleepnow` use logind/loginctl/Hyprland and
  normally run as the logged-in user.

## Not ported

These settings have no reliable or straightforward Linux equivalent and are
rejected rather than guessed at:

`disksleep`/`spindown`, `hibernatemode`, `hibernatefile`, `autorestart`,
`halfdim`, `lessbright`, `acwake`, `ring`, `sms`, `ttyskeepawake`,
`destroyfvkeyonstandby`, `autopoweroff`, `autopoweroffdelay`.

Scheduled and repeating power events (`schedule`, `repeat`, `relative`) are also
not implemented. Use systemd timers or `rtcwake(8)`.

UPS emergency thresholds (`-u`, `haltlevel`, `haltafter`, `haltremain`) are not
implemented; use UPower or NUT.

## Files

- `~/.config/omarchy/shell.json` — Omarchy display/idle timers
- `/etc/systemd/logind.conf.d/99-pmset.conf` — idle suspend and lid settings
- `/sys/class/power_supply/*` — battery and AC state
- `/sys/power/state` — suspend/hibernate capability detection

## License

MIT
