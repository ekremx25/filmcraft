//! Opt-in application diagnostics; no dependency on an external logging process.
use std::io::Write;
struct Logger;
impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info && metadata.target().starts_with("filmcraft")
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(std::io::stderr().lock(), "{} {}: {}", record.level(), record.target(), record.args());
        }
    }
    fn flush(&self) {}
}
pub fn install() {
    static LOGGER: Logger = Logger;
    if std::env::var("FILMCRAFT_LOG").is_ok_and(|v| v == "info") && log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
