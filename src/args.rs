use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::filter::LevelFilter;

#[derive(Debug, Clone, ValueEnum, Default)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
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

#[derive(Debug, Clone, Subcommand)]
#[cfg_attr(test, derive(serde::Serialize))]
pub enum Mode {
    /// Scan documents in a loop
    Scan,
    /// Process scanned documents (post-process TIFFs and run OCR)
    Process {
        /// Optional timestamp of a single directory to process (format: YYYYMMDD-HHMMSS)
        #[arg(value_name = "TIMESTAMP")]
        timestamp: Option<String>,
    },
    /// Archive processed documents
    Archive {
        /// Optional timestamp of a single directory to archive (format: YYYYMMDD-HHMMSS)
        #[arg(value_name = "TIMESTAMP")]
        timestamp: Option<String>,
    },
    /// Scan, process and archive a single document
    Single,
    /// Initialize the configuration file
    InitConfig,
}

#[derive(Parser, Debug)]
#[cfg_attr(test, derive(serde::Serialize))]
#[command(author, version, about, long_about = None)]
#[command(next_line_help = true)]
pub struct Args {
    /// Processing mode
    #[command(subcommand)]
    pub mode: Mode,

    /// Log level
    #[arg(short, long, value_enum, default_value_t = LogLevel::default(), global = true)]
    pub log_level: LogLevel,

    /// Dev mode: Don't actually scan, but use simulated scan TIFFs
    #[cfg_attr(not(debug_assertions), arg(skip))]
    #[cfg_attr(debug_assertions, arg(long, global = true))]
    pub fake_scan: bool,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::{Args, LogLevel, Mode};

    mod help_text {
        use insta::assert_snapshot;

        use super::*;

        #[test]
        fn main_help() {
            let mut cmd = Args::command();
            let help = cmd.render_help().to_string();
            assert_snapshot!(help);
        }
    }

    mod parse_args {
        use clap::Parser;
        use rstest::rstest;

        use super::*;

        #[test]
        fn scan_mode() {
            let args = Args::parse_from(["arkivisto", "scan"]);
            assert!(matches!(args.mode, Mode::Scan));
        }

        #[test]
        fn process_mode() {
            let args = Args::parse_from(["arkivisto", "process"]);
            assert!(matches!(args.mode, Mode::Process { timestamp: None }));
        }

        #[test]
        fn process_mode_with_timestamp() {
            let args = Args::parse_from(["arkivisto", "process", "20260602-205254"]);
            assert!(matches!(
                args.mode,
                Mode::Process { timestamp: Some(ts) } if ts == "20260602-205254"
            ));
        }

        #[test]
        fn archive_mode() {
            let args = Args::parse_from(["arkivisto", "archive"]);
            assert!(matches!(args.mode, Mode::Archive { timestamp: None }));
        }

        #[test]
        fn archive_mode_with_timestamp() {
            let args = Args::parse_from(["arkivisto", "archive", "20260602-205254"]);
            assert!(matches!(
                args.mode,
                Mode::Archive { timestamp: Some(ts) } if ts == "20260602-205254"
            ));
        }

        #[test]
        fn single_mode() {
            let args = Args::parse_from(["arkivisto", "single"]);
            assert!(matches!(args.mode, Mode::Single));
        }

        #[test]
        fn single_mode_rejects_timestamp() {
            let result = Args::try_parse_from(["arkivisto", "single", "20260602-205254"]);
            assert!(result.is_err());
        }

        #[rstest]
        #[case("trace", LogLevel::Trace)]
        #[case("debug", LogLevel::Debug)]
        #[case("info", LogLevel::Info)]
        #[case("warn", LogLevel::Warn)]
        #[case("error", LogLevel::Error)]
        fn log_level(#[case] input: &str, #[case] expected: LogLevel) {
            let args = Args::parse_from(["arkivisto", "scan", "-l", input]);
            assert!(
                matches!(args.log_level, ref level if std::mem::discriminant(level) == std::mem::discriminant(&expected))
            );
        }
    }

    mod error_messages {
        use clap::Parser;
        use insta::assert_snapshot;

        use super::*;

        #[test]
        fn invalid_mode() {
            let result = Args::try_parse_from(["arkivisto", "invalid_mode"]);
            let err = result.unwrap_err();
            assert_snapshot!(err.to_string());
        }

        #[test]
        fn invalid_log_level() {
            let result = Args::try_parse_from(["arkivisto", "scan", "-l", "invalid"]);
            let err = result.unwrap_err();
            assert_snapshot!(err.to_string());
        }
    }
}
