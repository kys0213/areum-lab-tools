use clap::Parser;

/// Minimal placeholder CLI for the workspace scaffold.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Name to greet
    #[arg(long, default_value = "world")]
    name: String,
}

fn greeting(name: &str) -> String {
    format!("Hello, {name}!")
}

fn main() {
    let cli = Cli::parse();
    println!("{}", greeting(&cli.name));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_includes_name() {
        assert_eq!(greeting("test"), "Hello, test!");
    }
}
