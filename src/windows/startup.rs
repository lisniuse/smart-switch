use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, ERROR_UNSUPPORTED_TYPE,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::core::PCWSTR;
use windows_sys::w;

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const VALUE_NAME: PCWSTR = w!("Smart Switch");
const MAX_RUN_COMMAND: usize = 260;

pub(super) fn enabled() -> io::Result<bool> {
    let expected = command_for(std::env::current_exe()?.as_os_str());
    let mut actual = vec![0u16; expected.len()];
    let mut bytes = size_of_val(actual.as_slice()) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE_NAME,
            RRF_RT_REG_SZ,
            null_mut(),
            actual.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    match result {
        ERROR_SUCCESS => Ok(actual == expected),
        ERROR_FILE_NOT_FOUND | ERROR_MORE_DATA | ERROR_UNSUPPORTED_TYPE => Ok(false),
        error => Err(io::Error::from_raw_os_error(error as i32)),
    }
}

pub(super) fn set_enabled(enabled: bool) -> io::Result<()> {
    let result = if enabled {
        let command = command_for(std::env::current_exe()?.as_os_str());
        if command.len() - 1 > MAX_RUN_COMMAND {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "startup path is too long",
            ));
        }
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE_NAME,
                REG_SZ,
                command.as_ptr().cast(),
                size_of_val(command.as_slice()) as u32,
            )
        }
    } else {
        let result = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME) };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        result
    };
    match result {
        ERROR_SUCCESS => Ok(()),
        error => Err(io::Error::from_raw_os_error(error as i32)),
    }
}

fn command_for(path: &OsStr) -> Vec<u16> {
    // Quoting preserves executable paths containing spaces.
    [b'"' as u16]
        .into_iter()
        .chain(path.encode_wide())
        .chain([b'"' as u16, 0])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_the_unicode_executable_path() {
        let path = OsStr::new(r"C:\应用 工具\smart-switch.exe");
        let expected: Vec<u16> = "\"C:\\应用 工具\\smart-switch.exe\"\0"
            .encode_utf16()
            .collect();
        assert_eq!(command_for(path), expected);
    }
}
