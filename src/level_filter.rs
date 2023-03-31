use clap_verbosity_flag::{LevelFilter, LogLevel, Verbosity};

pub(crate) struct VerbosityLevelFilter<T: LogLevel>(pub(crate) Verbosity<T>);

impl<T> From<VerbosityLevelFilter<T>> for tracing::level_filters::LevelFilter
where
    T: LogLevel,
{
    fn from(value: VerbosityLevelFilter<T>) -> Self {
        match value.0.log_level_filter() {
            LevelFilter::Off => Self::OFF,
            LevelFilter::Error => Self::ERROR,
            LevelFilter::Warn => Self::WARN,
            LevelFilter::Info => Self::INFO,
            LevelFilter::Debug => Self::DEBUG,
            LevelFilter::Trace => Self::TRACE,
        }
    }
}
