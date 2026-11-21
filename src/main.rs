#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    if windows::run().is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Smart Switch 需要 Windows 10 或 11。");
    std::process::exit(1);
}
