//! The compile worker as a standalone program, for the worker tests and
//! manual debugging (the app runs the same loop with `--compile-worker`).
//! Speaks the framed protocol of `lethora_typeset::worker` on stdin/stdout.

fn main() {
    if let Err(e) = lethora_typeset::worker::run(std::io::stdin(), std::io::stdout()) {
        eprintln!("lethora worker: {e}");
        std::process::exit(1);
    }
}
