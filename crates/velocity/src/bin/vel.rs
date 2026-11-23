//! Windows console client. It never initializes the editor or graphics.
#[cfg(windows)]
#[path = "../cli.rs"]
#[allow(dead_code)] // GUI-only options share this parser.
mod cli;
#[cfg(windows)]
#[path = "../cli_console.rs"]
mod cli_console;
#[cfg(windows)]
#[path = "../cli_stdin.rs"]
mod cli_stdin;
#[cfg(windows)]
#[path = "../instance_path.rs"]
mod instance_path;
#[cfg(windows)]
#[path = "../instance/pipe.rs"]
#[allow(dead_code)] // The GUI owns the listening side of the transport.
mod instance_pipe;
#[cfg(windows)]
#[path = "../instance_protocol.rs"]
#[allow(dead_code)] // Some protocol fields are only read by the primary.
mod instance_protocol;
#[cfg(windows)]
#[path = "../windows_gui_spawn.rs"]
mod windows_gui_spawn;
#[cfg(windows)]
#[path = "../windows_launch.rs"]
#[allow(dead_code)]
mod windows_launch;

#[cfg(windows)]
fn main() {
    match run() {
        Ok(status) => std::process::exit(status),
        Err(error) => {
            eprintln!("vel: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("vel.exe is the Windows console client. On macOS use velocity.");
    std::process::exit(1);
}

#[cfg(windows)]
fn run() -> Result<i32, String> {
    use cli::{Mode, Options};
    use instance_protocol::{Forward, Request, TIMEOUT};
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    let Some(options) = Options::parse(std::env::args().skip(1))? else {
        print!("{}", cli::HELP.replace("Usage: velocity", "Usage: vel"));
        return Ok(0);
    };
    if options.version {
        println!("Velocity {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if options.bench_help {
        print!("{}{}", cli::BENCH_HELP, cli::PERF_HELP);
        return Ok(0);
    }
    let gui = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("velocity.exe");
    if options.mode != Mode::Interactive {
        // Headless tooling still uses the main binary, with inherited streams.
        return Command::new(gui)
            .args(std::env::args_os().skip(1))
            .status()
            .map(|status| status.code().unwrap_or(1))
            .map_err(|e| e.to_string());
    }

    let normal = options
        .data_dir
        .clone()
        .or_else(|| {
            vel_config::paths::windows_data_dir(
                std::env::var_os("VELOCITY_DATA_DIR").as_deref(),
                std::env::var_os("APPDATA").as_deref(),
            )
        })
        .map(std::path::absolute)
        .transpose()
        .map_err(|e| e.to_string())?;
    let safe = if options.safe_mode {
        Some(cli::safe_profile(
            normal
                .as_deref()
                .ok_or("safe mode requires a data directory")?,
        ))
    } else {
        None
    };
    let request = Request::new(&options, cli_stdin::read(&options, false, false)?)?;
    if request.wait {
        cli_console::install_client_interrupt().map_err(|e| e.to_string())?;
    }
    let endpoint = instance_pipe::Endpoint::for_profile(safe.as_deref().or(normal.as_deref()))
        .map_err(|e| e.to_string())?;
    match endpoint.forward(&request)? {
        Forward::Submitted(result) => return result.map(|()| 0),
        Forward::NotRunning => {}
    }

    let mut command = Command::new(&gui);
    command.arg(windows_launch::SERVE).args([
        "--width",
        &options.width.to_string(),
        "--height",
        &options.height.to_string(),
        "--groups",
        &options.groups.to_string(),
    ]);
    if let Some(normal) = normal {
        command.arg("--data-dir").arg(normal);
    }
    if options.safe_mode {
        command.arg("--safe-mode");
    }
    if options.background {
        command.arg("--background");
    }
    if options.log_input {
        command.arg("--log-input");
    }
    if options.restore_session {
        command.arg("--restore-session");
    }
    if let Some(output) = options.output {
        command.arg("--output").arg(output);
    }
    let child = windows_gui_spawn::spawn(&command)
        .map_err(|e| format!("could not launch {}: {e}", gui.display()))?;
    let until = Instant::now() + TIMEOUT;
    loop {
        match endpoint.forward(&request)? {
            Forward::Submitted(result) => return result.map(|()| 0),
            Forward::NotRunning => {}
        }
        // A successful exit can be a competing GUI which lost primary election.
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())?
            && !status.success()
        {
            return Err(format!(
                "Velocity exited before accepting the request: {status}"
            ));
        }
        if Instant::now() >= until {
            return Err("Velocity did not open its local pipe".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
