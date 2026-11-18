#![cfg(windows)]
use std::io::{Read, Write};
use std::process::{Command, Stdio};

#[test]
fn console_probe() {
    if std::env::var_os("INSTANCE_CONSOLE_PROBE").is_none() {
        return;
    }
    let window = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
    assert!(window.is_null(), "A captured helper acquired a console");
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    assert_eq!(input, "fixture café 日本\n");
    std::io::stdout()
        .write_all(b"protocol-output-preserved\n")
        .unwrap();
    std::io::stderr()
        .write_all(b"error-output-preserved\n")
        .unwrap();
}

#[test]
fn captured_helpers_have_no_console_and_keep_all_protocol_streams() {
    let mut command = Command::new(std::env::current_exe().unwrap());
    agent_instance::platform::no_console_window(&mut command);
    let mut child = command
        .args(["--exact", "console_probe", "--nocapture"])
        .env("INSTANCE_CONSOLE_PROBE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("fixture café 日本\n".as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("protocol-output-preserved"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("error-output-preserved"));
}
