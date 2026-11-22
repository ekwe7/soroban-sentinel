use std::fs::{self, OpenOptions};
use std::io::{self, Write};

use sentinel_core::adapters::parse_contract_id;

use crate::args::InitArgs;
use crate::commands::{resolve_threshold_config, EXIT_INVALID_ARGS, EXIT_OK};

fn prompt(label: &str) -> Result<String, String> {
    print!("{label}: ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|e| e.to_string())?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(value)
    }
}

pub fn run_init(mut args: InitArgs) -> i32 {
    if args.output.exists() && !args.force {
        eprintln!(
            "Error: output '{}' already exists; pass --force to replace it",
            args.output.display()
        );
        return EXIT_INVALID_ARGS;
    }
    if !args.non_interactive && args.contracts.is_empty() {
        match prompt("Contract ID") {
            Ok(id) => args.contracts.push(id),
            Err(error) => {
                eprintln!("Error: {error}");
                return EXIT_INVALID_ARGS;
            }
        }
    }
    if args.contracts.is_empty() {
        eprintln!("Error: provide at least one --contract in non-interactive mode");
        return EXIT_INVALID_ARGS;
    }
    if let Err(error) = crate::commands::resolve_rpc_url(&args.network, None) {
        eprintln!("Error: {error}");
        return EXIT_INVALID_ARGS;
    }
    if let Err(error) = resolve_threshold_config(args.warning_threshold, args.critical_threshold) {
        eprintln!("Error: {error}");
        return EXIT_INVALID_ARGS;
    }
    for id in &args.contracts {
        if let Err(error) = parse_contract_id(id) {
            eprintln!("Error: invalid contract ID '{id}': {error}");
            return EXIT_INVALID_ARGS;
        }
    }
    let critical = args.critical_threshold.unwrap_or_else(|| {
        resolve_threshold_config(args.warning_threshold, None)
            .unwrap()
            .critical_ledgers
    });
    let mut manifest = format!(
        "[defaults]\nnetwork = {}\nwarning_threshold = {}\ncritical_threshold = {}\n\n",
        toml_string(&args.network),
        args.warning_threshold,
        critical
    );
    for id in &args.contracts {
        manifest.push_str("[[contracts]]\n");
        manifest.push_str(&format!("id = {}\n", toml_string(id)));
        if !args.keys.is_empty() {
            let keys = args
                .keys
                .iter()
                .map(|key| toml_string(key))
                .collect::<Vec<_>>()
                .join(", ");
            manifest.push_str(&format!("keys = [{keys}]\n"));
        }
        manifest.push('\n');
    }
    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            eprintln!(
                "Error: output directory '{}' does not exist",
                parent.display()
            );
            return EXIT_INVALID_ARGS;
        }
    }
    let result = if args.force {
        fs::write(&args.output, manifest)
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args.output)
            .and_then(|mut file| file.write_all(manifest.as_bytes()))
    };
    match result {
        Ok(()) => {
            println!("Wrote manifest to {}", args.output.display());
            EXIT_OK
        }
        Err(error) => {
            eprintln!(
                "Error: could not write '{}': {error}",
                args.output.display()
            );
            EXIT_INVALID_ARGS
        }
    }
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
