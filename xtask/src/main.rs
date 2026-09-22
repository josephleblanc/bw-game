//! Workspace automation for bw-game. All perf tooling lives here so that CI
//! and developers run byte-for-byte identical commands (ADR 0001, D1).

mod budgets;
mod perf;
mod record;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match perf::dispatch(&args) {
        Ok(code) => std::process::exit(i32::from(code)),
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::exit(2);
        }
    }
}
