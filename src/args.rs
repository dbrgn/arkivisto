use clap::{Parser, ValueEnum};
use tracing_subscriber::filter::LevelFilter;

#[derive(Debug, Clone, ValueEnum)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn to_filter(&self) -> LevelFilter {
        match self {
            LogLevel::Trace => LevelFilter::TRACE,
            LogLevel::Debug => LevelFilter::DEBUG,
            LogLevel::Info => LevelFilter::INFO,
            LogLevel::Warn => LevelFilter::WARN,
            LogLevel::Error => LevelFilter::ERROR,
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };
        write!(f, "{}", s)
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

#[derive(Debug, Clone, ValueEnum)]
pub enum Mode {
    Scan,
    Process,
    Archive,
    Single,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Single
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(next_line_help = true)]
pub struct Args {
    /// Processing mode
    #[arg(value_enum, default_value_t = Mode::default())]
    pub mode: Mode,

    /// Log level
    #[arg(short, long, value_enum, default_value_t = LogLevel::default())]
    pub log_level: LogLevel,

    /// Dev mode: Don't actually scan, but use simulated scan TIFFs
    #[cfg_attr(not(debug_assertions), arg(skip))]
    #[cfg_attr(debug_assertions, arg(long))]
    pub fake_scan: bool,
}
