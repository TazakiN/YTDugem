use anyhow::{Context, Result};
use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

pub fn configure() -> Result<()> {
    let exe = std::env::current_exe()?;
    let tools = exe
        .parent()
        .context("Cannot find the application folder")?
        .join("tools");
    if tools.is_dir() {
        let path = prepend_tools(&tools, std::env::var_os("PATH").as_deref())?;
        // Set before creating any worker threads; only this app and its children inherit it.
        std::env::set_var("PATH", path);
    }
    Ok(())
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
}
