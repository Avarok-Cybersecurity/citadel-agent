use super::*;

fn limits(lines: u32, window_ms: u64) -> BrowserLimits {
    BrowserLimits::new(lines, window_ms, 10, 64).unwrap()
}

fn line(message: &str, stack: Option<&str>) -> BrowserConsoleEntry {
    BrowserConsoleEntry {
        source: BrowserConsoleSource::ConsoleError,
        message: message.into(),
        stack: stack.map(Into::into),
        repeats: 1,
    }
}

#[test]
fn every_bound_is_validated() {
    assert!(BrowserLimits::new(0, 1, 1, 1).is_err());
    assert!(BrowserLimits::new(1, 0, 1, 1).is_err());
    assert!(BrowserLimits::new(1, 1, 0, 1).is_err());
    assert!(BrowserLimits::new(1, 1, 1, 0).is_err());
    assert!(BrowserLimits::new(1, 1, 1, 1).is_ok());
}

#[test]
fn the_window_admits_its_count_then_refuses_until_it_closes() {
    let limits = limits(2, 1_000);
    let mut pacer = BrowserPacer::default();
    assert!(pacer.admit(10, &limits));
    assert!(pacer.admit(500, &limits));
    assert!(
        !pacer.admit(1_009, &limits),
        "still inside the first window"
    );
    assert!(pacer.admit(1_010, &limits), "a new window opened at 1_010");
}

#[test]
fn a_clock_that_goes_back_opens_a_new_window() {
    let limits = limits(1, 1_000);
    let mut pacer = BrowserPacer::default();
    assert!(pacer.admit(5_000, &limits));
    assert!(!pacer.admit(5_001, &limits));
    assert!(pacer.admit(4_000, &limits));
}

#[test]
fn extension_lines_are_not_acceptable() {
    let limits = limits(1, 1);
    for (message, stack) in [
        (
            "TypeError: x is undefined",
            Some("at f (chrome-extension://abc/content.js:1:2)"),
        ),
        (
            "Uncaught Error",
            Some("@moz-extension://9f1e/inject.js:3:4"),
        ),
        ("Error", Some("safari-web-extension://AB12/script.js:1:1")),
        ("Grammarly.js:2 Uncaught (in promise) Error", None),
        ("from CHROME-EXTENSION://x/y.js", None),
    ] {
        assert!(
            !acceptable(&line(message, stack), &limits),
            "{message:?} kept"
        );
    }
    assert!(acceptable(
        &line(
            "TypeError: x",
            Some("at f (https://work.example/assets/app.js:1:2)")
        ),
        &limits
    ));
}

#[test]
fn an_oversized_field_drops_the_line_instead_of_being_cut() {
    let limits = limits(1, 1);
    assert!(acceptable(&line(&"a".repeat(64), None), &limits));
    assert!(!acceptable(&line(&"a".repeat(65), None), &limits));
    assert!(!acceptable(&line("short", Some(&"b".repeat(65))), &limits));
}

#[test]
fn the_source_decides_target_and_level() {
    use crate::telemetry::entry::TelemetryLevel::{Error, Warn};
    assert_eq!(
        classify(BrowserConsoleSource::ConsoleError),
        ("browser.console", Error)
    );
    assert_eq!(
        classify(BrowserConsoleSource::ConsoleWarn),
        ("browser.console", Warn)
    );
    assert_eq!(
        classify(BrowserConsoleSource::WindowError),
        ("browser.error", Error)
    );
    assert_eq!(
        classify(BrowserConsoleSource::UnhandledRejection),
        ("browser.rejection", Error)
    );
}

#[test]
fn the_text_says_how_often_it_repeated_then_where() {
    let mut entry = line("boom", Some("at f (a.js:1:2)"));
    assert_eq!(text(&entry), "boom\nat f (a.js:1:2)");
    entry.repeats = 4;
    entry.stack = Some("  ".into());
    assert_eq!(text(&entry), "boom (repeated 4 times)");
}
