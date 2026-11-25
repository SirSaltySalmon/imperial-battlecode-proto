use ibc_worker::config::{parse_args, USAGE};
use ibc_worker::{log, Shutdown};
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("ibc-worker {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let cfg = match parse_args(&args) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("ibc-worker: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    // SIGTERM / SIGINT / Ctrl-C: the first drains, the second aborts.
    let shutdown = Arc::new(Shutdown::default());
    let on_signal = shutdown.clone();
    let installed = ctrlc::set_handler(move || {
        if on_signal.is_draining() {
            log!("warn", "second signal: aborting");
            on_signal.request_abort();
        } else {
            log!("info", "signal received: draining");
            on_signal.request_drain();
        }
    });
    if let Err(e) = installed {
        log!(
            "error",
            "cannot install signal handlers",
            error = e.to_string()
        );
    }

    match ibc_worker::run(cfg, shutdown) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            log!("error", "worker failed", error = e);
            ExitCode::FAILURE
        }
    }
}
