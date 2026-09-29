use std::io::IsTerminal;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let is_tty = stdout.is_terminal();
    std::process::exit(pulse::run_streaming(
        &args,
        Box::new(stdout),
        Box::new(std::io::stderr()),
        is_tty,
    ));
}
