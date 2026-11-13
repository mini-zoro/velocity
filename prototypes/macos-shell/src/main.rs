//! Standalone macOS window/render/IME experiment; not a product crate.
mod app;
mod atlas;
mod cli;
#[cfg(test)]
mod cpu_tests;
mod editor;
mod font;
mod layout;
mod lexer;
mod metal;
mod native;
mod render;
mod report;
mod stats;

fn main() -> std::process::ExitCode {
    // Keep this first: process-relative values include dynamic loader work before main.
    let entered = std::time::Instant::now();
    let wall_at_entry = std::time::SystemTime::now();
    let options = match cli::Options::parse(std::env::args().skip(1)) {
        Ok(Some(o)) => o,
        Ok(None) => {
            print!("{}", cli::HELP);
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n{}", cli::HELP);
            return std::process::ExitCode::from(2);
        }
    };
    let mut clock = report::Clock::new(entered, wall_at_entry);
    let reporter = match report::Reporter::new(options.mode, options.output.as_deref(), entered) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return std::process::ExitCode::from(2);
        }
    };
    let previous_hook = std::panic::take_hook();
    let panic_reporter = reporter.clone();
    std::panic::set_hook(Box::new(move |info| {
        let _=panic_reporter.emit(serde_json::json!({"kind":"error","stage":"panic","message":info.to_string(),"complete":false}));
        previous_hook(info);
    }));
    let _watchdog = options
        .mode
        .automatic()
        .then(|| report::Watchdog::start(options.timeout_seconds, reporter.clone()));
    clock.mark("measurement_ready");
    let result = if options.mode == cli::Mode::Metal {
        metal::run(options, clock, reporter.clone())
    } else {
        app::run(options, clock, reporter.clone())
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            let _ = reporter.emit(
                serde_json::json!({"kind":"error","stage":"run","message":e,"complete":false}),
            );
            eprintln!("vel-shell: {e}");
            std::process::ExitCode::from(1)
        }
    }
}
