//! Fixed release-package setup. No shell, PATH interpreter or remote commands.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

fn ordinary(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(!relative.contains(['\\', ':']), "Invalid bundle path");
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("Invalid bundle path");
        };
        path.push(name);
        ensure!(!path.is_symlink(), "Bundle links are not accepted");
    }
    ensure!(path.is_file(), "The consumer bundle is incomplete");
    Ok(path)
}

pub fn run(home: &Path, arguments: Vec<String>) -> Result<()> {
    let executable = std::env::current_exe()?;
    let root = executable
        .parent()
        .context("Executable directory unavailable")?;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join("consumer-bundle.json"))
            .context("Setup needs the complete consumer package beside this executable")?,
    )?;
    ensure!(
        manifest["schema"] == 1 && manifest["kind"] == "instance-consumer-bundle",
        "Unsupported consumer bundle"
    );
    let files = manifest["files"]
        .as_object()
        .context("Bundle inventory missing")?;
    for (name, expected) in files {
        let path = ordinary(root, name)?;
        let actual = format!("{:x}", Sha256::digest(std::fs::read(path)?));
        ensure!(
            expected.as_str() == Some(actual.as_str()),
            "Bundle checksum mismatch: {name}"
        );
    }
    let binary_name = crate::platform::executable("agent-instance");
    ensure!(
        manifest["binary"] == binary_name && files.contains_key(&binary_name),
        "Bundle executable mismatch"
    );
    let lock: Value = serde_json::from_str(include_str!("../scripts/vendor/python/lock.json"))?;
    let directory = lock["directory"].as_str().unwrap();
    ensure!(
        manifest["python_directory"] == directory,
        "Bundle interpreter mismatch"
    );
    let relative = format!(
        "support/{directory}/{}",
        if cfg!(windows) {
            "python.exe"
        } else {
            "bin/python3.13"
        }
    );
    let script = "setup/native-install.py";
    ensure!(
        files.contains_key(&relative)
            && files.contains_key(script)
            && files.contains_key("setup/consumer-bundle.py"),
        "Bundle setup files missing"
    );
    let mut command = std::process::Command::new(ordinary(root, &relative)?);
    command
        .args(["-I", "-X", "utf8"])
        .arg("-B")
        .arg(ordinary(root, script)?)
        .args(arguments)
        .arg("--home")
        .arg(home)
        .arg("--binary")
        .arg(&executable)
        .arg("--bundle")
        .arg(root);
    ensure!(
        command.status()?.success(),
        "Instance setup did not complete; existing state was preserved"
    );
    Ok(())
}
