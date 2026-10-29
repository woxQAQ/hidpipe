//! Runtime logging with a local timestamp, format [HH:MM:SS].
//! main.rs pulls this in with `#[macro_use] mod log;` so every module can
//! call `log!`.

pub fn timestamp() -> String {
    let fmt = time::macros::format_description!("[hour]:[minute]:[second]");
    time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .format(&fmt)
        .unwrap_or_default()
}

macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("[{}] {}", $crate::log::timestamp(), format_args!($($arg)*))
    };
}
