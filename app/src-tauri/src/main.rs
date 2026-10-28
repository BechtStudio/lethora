// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The compile worker: the same binary, typesetting on stdin/stdout.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == lethora_app::WORKER_FLAG)
    {
        let result = lethora_typeset::worker::run(std::io::stdin(), std::io::stdout());
        if let Err(e) = &result {
            eprintln!("lethora worker: {e}");
        }
        std::process::exit(i32::from(result.is_err()));
    }
    lethora_app::run()
}
