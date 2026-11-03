use anyhow::{Context, Result};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

const MPV: &str = if cfg!(windows) { "mpv.exe" } else { "mpv" };

pub fn configure() -> Result<()> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("Cannot find the application folder")?;
    if let Some(tools) = find_tools(dir) {
        let path = prepend_tools(&tools, std::env::var_os("PATH").as_deref())?;
        // Set before creating any worker threads; only this app and its children inherit it.
        std::env::set_var("PATH", path);
    }
    Ok(())
}

/// The portable ZIP keeps `tools` beside the executable; that always wins. A build run straight
/// from `target/<profile>/` has no `tools` folder, so fall back to the project's bundled
/// `dist/tools` (the same SHA-256-verified runtime the packager ships), at most three folders up.
fn find_tools(exe_dir: &Path) -> Option<PathBuf> {
    let beside = exe_dir.join("tools");
    if beside.is_dir() {
        return Some(beside);
    }
    candidates(exe_dir)
        .into_iter()
        .skip(1)
        .find(|tools| tools.join(MPV).is_file())
}

/// Every folder searched, in order (like the original launcher's `findJvmLib` candidate list).
fn candidates(exe_dir: &Path) -> Vec<PathBuf> {
    std::iter::once(exe_dir.join("tools"))
        .chain(
            exe_dir
                .ancestors()
                .skip(1)
                .take(3)
                .map(|dir| dir.join("dist").join("tools")),
        )
        .collect()
}

/// Names the exact folders searched, as the original's `Failed to find JVM in "<dir>" directory.`
pub fn missing_tool_message(tool: &str) -> String {
    let searched = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(candidates))
        .unwrap_or_default()
        .iter()
        .map(|dir| format!("\"{}\"", dir.display()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Starting {tool}: {tool} was not found in {searched}. Extract the COMPLETE YTDugem ZIP, keeping the tools folder beside YTDugem.exe, or set BITCHORD_MPV"
    )
}

fn prepend_tools(tools: &Path, old: Option<&OsStr>) -> Result<OsString> {
    let mut paths = vec![tools.to_owned()];
    if let Some(old) = old {
        paths.extend(std::env::split_paths(old));
    }
    Ok(std::env::join_paths(paths)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_tools_work_without_a_system_install() {
        let tools = std::env::temp_dir()
            .join("BitChord portable with spaces")
            .join("tools");
        let system = std::env::join_paths([std::env::temp_dir().join("system")]).unwrap();
        let path = prepend_tools(&tools, Some(&system)).unwrap();
        let paths: Vec<_> = std::env::split_paths(&path).collect();
        assert_eq!(paths[0], tools);
        assert_eq!(paths.len(), 2);
        assert_eq!(
            std::env::split_paths(&prepend_tools(&tools, None).unwrap()).collect::<Vec<_>>(),
            vec![tools]
        );
    }
    #[test]
    fn build_output_finds_project_tools_but_portable_tools_win() {
        let root = std::env::temp_dir().join(format!("ytdugem-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let release = root.join("target").join("release");
        let dist = root.join("dist").join("tools");
        std::fs::create_dir_all(&release).unwrap();
        assert_eq!(find_tools(&release), None, "no mpv anywhere");
        std::fs::create_dir_all(&dist).unwrap();
        assert_eq!(
            find_tools(&release),
            None,
            "dist/tools without mpv is ignored"
        );
        std::fs::write(dist.join(MPV), b"").unwrap();
        assert_eq!(find_tools(&release), Some(dist.clone()));
        std::fs::create_dir_all(release.join("tools")).unwrap();
        assert_eq!(find_tools(&release), Some(release.join("tools")));
        let searched = candidates(&release);
        assert_eq!(searched[0], release.join("tools"));
        assert_eq!(
            searched[2], dist,
            "target/release -> project root/dist/tools"
        );
        assert_eq!(searched.len(), 4);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
