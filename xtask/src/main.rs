//! Std-only development tasks: `cargo xtask <ci|package|smoke>`.

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const USAGE: &str = "Usage: cargo xtask <command>

Commands:
  ci                         fmt check, clippy, tests, release build
  package [args...]          run scripts/package-windows.py (args forwarded)
  smoke [--ui] [--playback] [--online] [--real-audio]
                             Windows smoke tests (default: all three scripts)
  help                       show this message";

#[derive(Debug, PartialEq)]
struct Smoke {
    base: bool,
    ui: bool,
    playback: bool,
    online: bool,
    real_audio: bool,
}

fn parse_smoke(args: &[String]) -> Result<Smoke, String> {
    let (mut ui, mut playback, mut online, mut real_audio) = (false, false, false, false);
    for arg in args {
        match arg.as_str() {
            "--ui" => ui = true,
            "--playback" => playback = true,
            "--online" => online = true,
            "--real-audio" => real_audio = true,
            other => return Err(format!("unknown smoke option: {other}")),
        }
    }
    let all = !ui && !playback;
    Ok(Smoke {
        base: all,
        ui: ui || all,
        playback: playback || all,
        online,
        real_audio,
    })
}

fn root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(PathBuf::from).unwrap_or(manifest)
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    println!("> {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root())
        .status()
        .map_err(|e| format!("failed to start {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

fn ci() -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    run(&cargo, &["fmt", "--all", "--check"])?;
    run(
        &cargo,
        &["clippy", "--workspace", "--locked", "--", "-D", "warnings"],
    )?;
    run(&cargo, &["test", "--workspace", "--locked"])?;
    run(&cargo, &["build", "--release", "--locked"])
}

fn python() -> Result<(String, Vec<&'static str>), String> {
    if let Ok(custom) = env::var("PYTHON") {
        if !custom.is_empty() {
            return Ok((custom, vec![]));
        }
    }
    for (program, prefix) in [("python3", vec![]), ("python", vec![]), ("py", vec!["-3"])] {
        let ok = Command::new(program)
            .args(&prefix)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Ok((program.into(), prefix));
        }
    }
    Err("no working Python interpreter found (set PYTHON to override)".into())
}

fn package(extra: &[String]) -> Result<(), String> {
    let (program, prefix) = python()?;
    let mut args: Vec<&str> = prefix;
    args.push("scripts/package-windows.py");
    args.extend(extra.iter().map(String::as_str));
    run(&program, &args)
}

fn smoke(args: &[String]) -> Result<(), String> {
    let plan = parse_smoke(args)?;
    if !cfg!(windows) {
        return Err("smoke tests are only supported on Windows".into());
    }
    let script = |name: &str, extra: &[&str]| {
        let path = format!("scripts/{name}");
        let mut a = vec![
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            path.as_str(),
        ];
        a.extend_from_slice(extra);
        run("powershell", &a)
    };
    if plan.base {
        script("windows-smoke.ps1", &[])?;
    }
    if plan.ui {
        script("windows-ui-smoke.ps1", &[])?;
    }
    if plan.playback {
        let mut extra = vec![];
        if plan.online {
            extra.push("-Online");
        }
        if plan.real_audio {
            extra.push("-RealAudio");
        }
        script("windows-playback-smoke.ps1", &extra)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("package") => package(rest),
        Some("smoke") => smoke(rest),
        Some("help" | "--help" | "-h") | None => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn smoke_defaults_to_all() {
        let p = parse_smoke(&[]).unwrap();
        assert!(p.base && p.ui && p.playback && !p.online && !p.real_audio);
    }

    #[test]
    fn smoke_selectors_narrow() {
        let p = parse_smoke(&s(&["--playback", "--online", "--real-audio"])).unwrap();
        assert!(!p.base && !p.ui && p.playback && p.online && p.real_audio);
        let p = parse_smoke(&s(&["--ui"])).unwrap();
        assert!(!p.base && p.ui && !p.playback);
    }

    #[test]
    fn smoke_rejects_unknown() {
        assert!(parse_smoke(&s(&["--bogus"])).is_err());
    }

    #[test]
    fn root_has_workspace_manifest() {
        assert!(root().join("Cargo.toml").is_file());
    }
}
