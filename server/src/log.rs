//! 运行日志:统一带本地时间戳,格式 [HH:MM:SS]。
//! main.rs 以 `#[macro_use] mod log;` 引入,各模块直接用 `log!`。

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
