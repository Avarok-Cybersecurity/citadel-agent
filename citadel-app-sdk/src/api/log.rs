//! `log`: lines in the host's console for this app. The panic hook writes here.

use crate::call_now as call;
use citadel_app_sdk_types::calls::{LogLevel, LogWrite};
use citadel_app_sdk_types::Refused;

pub fn write(level: LogLevel, message: &str) -> Result<(), Refused> {
    call(&LogWrite {
        level,
        message: message.into(),
    })
}

pub fn debug(message: &str) -> Result<(), Refused> {
    write(LogLevel::Debug, message)
}

pub fn info(message: &str) -> Result<(), Refused> {
    write(LogLevel::Info, message)
}

pub fn warn(message: &str) -> Result<(), Refused> {
    write(LogLevel::Warn, message)
}

pub fn error(message: &str) -> Result<(), Refused> {
    write(LogLevel::Error, message)
}
