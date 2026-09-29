fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    std::process::exit(pulse::run(&args, &mut stdout, &mut stderr));
}
