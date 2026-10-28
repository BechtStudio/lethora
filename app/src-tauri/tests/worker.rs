//! The app binary doubles as the compile worker (`--compile-worker`).

use std::process::Command;

use lethora_typeset::layout::{self, Entries};
use lethora_typeset::{CompileRequest, Limits, Meta, Supervisor};

#[test]
fn app_binary_is_a_compile_worker() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lethora-app"));
    command.arg(lethora_app::WORKER_FLAG);
    let mut sup = Supervisor::new(command, Limits::default());
    let dir = std::env::temp_dir();
    let request = CompileRequest {
        project_dir: dir,
        layout: layout::resolve(&Entries::new()),
        meta: Meta::default(),
        custom: String::new(),
        content: "= Hello\n\nA page from the worker.".into(),
        today: None,
        overrides: Vec::new(),
        bibliography: String::new(),
        assets: Default::default(),
    };
    let compiled = sup.compile(7, request).unwrap();
    assert!(compiled.ok, "{:?}", compiled.diagnostics);
    assert_eq!(compiled.pages, 1);
    assert!(sup.render_page(7, 0).unwrap().starts_with(b"<svg"));
    assert!(sup.fonts().unwrap().families.len() > layout::fonts().len());
    sup.shutdown();
}
