// SPDX-License-Identifier: GPL-3.0-only
//! Direct USB control of OBSBOT cameras.

use std::process::ExitCode;

use libobsbot_core::{Devices, Error, ProductType};

const HELP: &str = "Control an OBSBOT Tiny 3 directly over USB.

Usage: obsbot [--device <ID>] <COMMAND>

Commands:
  devices          List connected supported cameras and their device IDs
  track            Read the current tracking state and toggle it
  track on|off     Enable or disable ordinary person tracking
  zoom             Read the current zoom factor
  zoom <FACTOR>    Set magnification (for example, 1.5 for 1.5×)
  standby          Put the camera in standby
  wake             Wake the camera and allow startup to settle

Options:
  --device <ID>    Select an ID printed by `obsbot devices`
  -h, --help       Show this help

One connected camera is selected automatically. With multiple cameras,
--device is required. Device IDs can change when a camera is reconnected.
Control commands print the camera's confirmed resulting state.
";

#[derive(Debug, PartialEq)]
struct Args {
    device: Option<String>,
    command: Command,
}

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Devices,
    Track(Option<bool>),
    Zoom(Option<f64>),
    Standby(bool),
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut args = args.into_iter();
    let mut device = None;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Ok(Args {
                    device,
                    command: Command::Help,
                });
            }
            "--device" => {
                if device.is_some() {
                    return Err("--device may only be specified once".into());
                }
                let id = args
                    .next()
                    .filter(|id| !id.is_empty() && !id.starts_with('-'))
                    .ok_or("--device requires a device ID from `obsbot devices`")?;
                device = Some(id);
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg:?}")),
            _ => positional.push(arg),
        }
    }
    let command = match positional.as_slice() {
        [] if device.is_none() => Command::Help,
        [] => return Err("expected a command; see `obsbot --help`".into()),
        [command] if command == "devices" => Command::Devices,
        [command] if command == "track" => Command::Track(None),
        [command, state] if command == "track" => match state.as_str() {
            "on" => Command::Track(Some(true)),
            "off" => Command::Track(Some(false)),
            _ => return Err("tracking state must be `on` or `off`".into()),
        },
        [command] if command == "zoom" => Command::Zoom(None),
        [command] if command == "standby" => Command::Standby(true),
        [command] if command == "wake" => Command::Standby(false),
        [command, factor] if command == "zoom" => {
            let factor = factor
                .parse::<f64>()
                .ok()
                .filter(|factor| factor.is_finite() && *factor > 0.0)
                .ok_or("zoom factor must be a finite positive number (for example, 1.5)")?;
            Command::Zoom(Some(factor))
        }
        [command, ..]
            if matches!(
                command.as_str(),
                "devices" | "track" | "zoom" | "standby" | "wake"
            ) =>
        {
            return Err(format!(
                "too many arguments for {command}; see `obsbot --help`"
            ));
        }
        [command, ..] => return Err(format!("unknown command {command:?}; see `obsbot --help`")),
    };
    Ok(Args { device, command })
}

fn run(args: &Args) -> Result<(), String> {
    if args.command == Command::Help {
        print!("{HELP}");
        return Ok(());
    }
    let devices = Devices::new().map_err(|err| err.to_string())?;
    if args.command == Command::Devices {
        let cameras = devices.list();
        if cameras.is_empty() {
            println!("No supported OBSBOT cameras connected.");
        }
        for info in cameras {
            let serial = if info.serial.is_empty() {
                devices
                    .open(&info)
                    .and_then(|device| device.serial_from_camera())
                    .unwrap_or_default()
            } else {
                info.serial.clone()
            };
            print!(
                "{}\t{}\tUSB {:04x}:{:04x}",
                info.id(),
                info.product_type.name(),
                info.vendor_id,
                info.product_id
            );
            for (label, value) in [
                ("product", &info.product_name),
                ("manufacturer", &info.manufacturer),
                ("serial", &serial),
            ] {
                if !value.is_empty() {
                    print!("\t{label}={value:?}");
                }
            }
            println!();
        }
        return Ok(());
    }
    let info = devices.select(args.device.as_deref()).map_err(|err| {
        format!("{err}; use `obsbot devices` and `--device <ID>` to select a camera")
    })?;
    if info.product_type != ProductType::Tiny3 {
        return Err(format!(
            "CLI controls are supported for Tiny 3; selected {}",
            info.product_type.name()
        ));
    }
    let device = devices.open(&info).map_err(|err| err.to_string())?;
    match args.command {
        Command::Track(state) => {
            let state = match state {
                Some(state) => device.set_tracking(state),
                None => device.toggle_tracking(),
            }
            .map_err(|err| match err {
                Error::Timeout => "camera did not confirm tracking before timeout".into(),
                _ => err.to_string(),
            })?;
            println!("tracking {}", if state { "on" } else { "off" });
        }
        Command::Zoom(factor) => {
            let result = match factor {
                Some(factor) => device.set_zoom_factor(factor),
                None => device.zoom_factor(),
            };
            let achieved = result.map_err(|err| match err {
                Error::Timeout => "camera did not confirm zoom before timeout".into(),
                Error::OutOfRange => match device.zoom_factor_range() {
                    Ok(range) => format!(
                        "zoom factor must be between {} and {}",
                        range.start(),
                        range.end()
                    ),
                    Err(_) => err.to_string(),
                },
                _ => err.to_string(),
            })?;
            println!("zoom {achieved}×");
        }
        Command::Standby(standby) => {
            let state = device.set_standby(standby).map_err(|err| match err {
                Error::Timeout => format!(
                    "camera did not confirm {} before timeout",
                    if standby { "standby" } else { "wake" }
                ),
                _ => err.to_string(),
            })?;
            println!("{}", if state { "standby" } else { "awake" });
        }
        Command::Help | Command::Devices => unreachable!("handled before opening a camera"),
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)).and_then(|args| run(&args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("obsbot: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn required_commands_and_global_device_option() {
        for (args, command) in [
            (vec!["devices"], Command::Devices),
            (vec!["track"], Command::Track(None)),
            (vec!["track", "on"], Command::Track(Some(true))),
            (vec!["track", "off"], Command::Track(Some(false))),
            (vec!["zoom"], Command::Zoom(None)),
            (vec!["zoom", "1.5"], Command::Zoom(Some(1.5))),
            (vec!["standby"], Command::Standby(true)),
            (vec!["wake"], Command::Standby(false)),
        ] {
            assert_eq!(parse(&args).unwrap().command, command);
        }
        for args in [
            vec!["--device", "usb-123", "zoom", "1.5"],
            vec!["zoom", "1.5", "--device", "usb-123"],
        ] {
            assert_eq!(
                parse(&args).unwrap(),
                Args {
                    device: Some("usb-123".into()),
                    command: Command::Zoom(Some(1.5)),
                }
            );
        }
        assert_eq!(
            parse(&["--device", "usb-123", "track"]).unwrap().command,
            Command::Track(None)
        );
        for (command, standby) in [("standby", true), ("wake", false)] {
            assert_eq!(
                parse(&["--device", "usb-123", command]).unwrap(),
                Args {
                    device: Some("usb-123".into()),
                    command: Command::Standby(standby),
                }
            );
        }
    }

    #[test]
    fn rejects_invalid_zoom_values_before_opening_hardware() {
        for value in ["NaN", "inf", "-inf", "1e999", "0", "-1", "1.5x", "", "abc"] {
            assert!(parse(&["zoom", value]).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn rejects_invalid_arguments() {
        for args in [
            vec!["--device"],
            vec!["--device", ""],
            vec!["--device", "--help"],
            vec!["--device", "usb-1"],
            vec!["--device", "usb-1", "--device", "usb-2", "track"],
            vec!["--unknown"],
            vec!["unknown"],
            vec!["track", "yes"],
            vec!["track", "on", "off"],
            vec!["zoom", "1", "2"],
            vec!["devices", "extra"],
            vec!["standby", "extra"],
            vec!["wake", "extra"],
        ] {
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn help_does_not_need_a_camera() {
        for args in [vec![], vec!["--help"], vec!["-h"], vec!["track", "--help"]] {
            assert_eq!(parse(&args).unwrap().command, Command::Help);
        }
    }
}
