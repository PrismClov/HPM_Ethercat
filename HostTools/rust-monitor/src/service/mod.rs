pub mod esi;
pub mod process;

use crate::backend::{Backend, Session, Update};
use std::{env, path::PathBuf, process::Command, thread, time::Duration};

pub type Result<T> = std::result::Result<T, String>;

pub fn cli() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

const USAGE: &str = "ethercat-service scan <igh|soem> <master|interface> --exclusive
ethercat-service process <igh|soem> <master|interface> <period-us> <freerun|AssignActivate> --exclusive [--allow-outputs]
ethercat-service esi <igh|soem> <master|interface> <position> <ESI.xml> <backup.sii> --exclusive [--write]
ethercat-service esi-plan <ESI.xml> <original.sii> <prepared.sii>";

fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if matches!(args.as_slice(), [] | [_]) && args.first().is_none_or(|v| v == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    if let [command, xml, original, prepared] = args.as_slice() {
        if command == "esi-plan" {
            return esi::prepare(xml, original, prepared);
        }
    }
    let [mode, backend, target, rest @ ..] = args.as_slice() else {
        return Err(USAGE.into());
    };
    let backend = match backend.as_str() {
        "igh" if cfg!(target_os = "linux") => Backend::Igh,
        "soem" => Backend::Soem,
        _ => return Err("Select SOEM or native Linux IgH.".into()),
    };
    if target.is_empty()
        || target.starts_with('-')
        || (backend == Backend::Igh && target.parse::<u32>().is_err())
    {
        return Err("Invalid interface or master index.".into());
    }
    if !rest.iter().any(|v| v == "--exclusive") {
        return Err(
            "Use --exclusive after stopping other applications and securing the bus.".into(),
        );
    }
    let params: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|v| *v != "--exclusive")
        .collect();
    match (mode.as_str(), params.as_slice()) {
        ("scan", []) => scan(backend, target),
        ("process", [period, sync, flags @ ..])
            if flags.is_empty() || *flags == ["--allow-outputs"] =>
        {
            let period = number(period)
                .filter(|v| (100..=20_000).contains(v))
                .ok_or("Period must be 100–20000 µs.")? as u32;
            let assign = if *sync == "freerun" {
                0
            } else {
                number(sync)
                    .filter(|v| (1..=u16::MAX as u64).contains(v))
                    .ok_or("Use freerun or the device's nonzero AssignActivate.")?
                    as u16
            };
            process::run(backend, target, period, assign, !flags.is_empty())
        }
        ("esi", [position, xml, backup, flags @ ..])
            if flags.is_empty() || *flags == ["--write"] =>
        {
            let position = number(position)
                .and_then(|v| u16::try_from(v).ok())
                .ok_or("Invalid position.")?;
            esi::run(backend, target, position, xml, backup, !flags.is_empty())
        }
        _ => Err(USAGE.into()),
    }
}

fn scan(backend: Backend, target: &str) -> Result<()> {
    let session = Session::start(
        backend,
        target.to_owned(),
        Duration::from_millis(250),
        false,
        || {},
    );
    for _ in 0..160 {
        for update in session.take_updates() {
            match update {
                Update::Snapshot { data, .. } if data.status != "scanning" => {
                    println!(
                        "{} · reported {} · listed {}",
                        data.status,
                        data.count,
                        data.slaves.len()
                    );
                    for s in data.slaves {
                        println!(
                            "{}\t{:08X}:{:08X}:{:08X}\tAL {:?}\tcode {:?}\t{}",
                            s.position, s.vendor, s.product, s.revision, s.state, s.al_code, s.name
                        );
                    }
                    return Ok(());
                }
                Update::Error(error) => return Err(error),
                Update::Stopped => return Err("Master stopped during scan.".into()),
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("Master scan timed out.".into())
}

fn number(text: &str) -> Option<u64> {
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("#x")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        text.parse().ok()
    }
}

fn helper(backend: Backend) -> Result<Command> {
    let executable = match backend {
        Backend::Igh => "ecat-monitor-igh",
        Backend::Soem => "ecat-monitor-soem",
        Backend::Demo => return Err("A physical backend is required.".into()),
    };
    let directory = env::var_os("ECAT_MONITOR_BACKEND_DIR")
        .map(PathBuf::from)
        .or_else(|| env::current_exe().ok()?.parent().map(PathBuf::from))
        .ok_or("Cannot resolve the native backend directory.")?;
    let mut command =
        Command::new(directory.join(format!("{executable}{}", env::consts::EXE_SUFFIX)));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    Ok(command)
}
