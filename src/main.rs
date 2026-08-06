use std::io::{self, Write};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    let stdout = io::stdout();
    let stderr = io::stderr();
    let stdin = io::stdin();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let mut input = stdin.lock();

    let code = {
        let mut io_handles = steamtrain::cli::Io {
            out: &mut out,
            err: &mut err,
            input: &mut input,
        };
        let deps = steamtrain::cli::Deps::default();
        steamtrain::cli::main(&argv, &mut io_handles, &deps)
    };

    let _ = out.flush();
    let _ = err.flush();
    std::process::exit(code);
}
