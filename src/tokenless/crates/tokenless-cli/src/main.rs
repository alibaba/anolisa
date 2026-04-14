use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Read};
use std::process;
use tokenless_schema::{ResponseCompressor, SchemaCompressor};

#[derive(Parser)]
#[command(
    name = "tokenless",
    version,
    about = "LLM token optimization via schema and response compression"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Compress OpenAI Function Calling tool schemas
    CompressSchema {
        /// Input file path (reads from stdin if omitted)
        #[arg(short, long)]
        file: Option<String>,
        /// Treat input as a JSON array of tools
        #[arg(long)]
        batch: bool,
    },
    /// Compress API responses
    CompressResponse {
        /// Input file path (reads from stdin if omitted)
        #[arg(short, long)]
        file: Option<String>,
    },
}

fn read_input(file: &Option<String>) -> Result<String, String> {
    match file {
        Some(path) => {
            fs::read_to_string(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))
        }
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            Ok(buf)
        }
    }
}

fn run() -> Result<(), (String, i32)> {
    let cli = Cli::parse();

    match cli.command {
        Commands::CompressSchema { file, batch } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value =
                serde_json::from_str(&input).map_err(|e| (format!("JSON parse error: {}", e), 1))?;

            let compressor = SchemaCompressor::new();

            if batch {
                let arr = value
                    .as_array()
                    .ok_or_else(|| ("Expected a JSON array for --batch mode".to_string(), 1))?;
                let results: Vec<serde_json::Value> =
                    arr.iter().map(|item| compressor.compress(item)).collect();
                let output = serde_json::to_string_pretty(&results)
                    .map_err(|e| (format!("Serialization error: {}", e), 2))?;
                println!("{}", output);
            } else {
                let result = compressor.compress(&value);
                let output = serde_json::to_string_pretty(&result)
                    .map_err(|e| (format!("Serialization error: {}", e), 2))?;
                println!("{}", output);
            }
        }
        Commands::CompressResponse { file } => {
            let input = read_input(&file).map_err(|e| (e, 2))?;
            let value: serde_json::Value =
                serde_json::from_str(&input).map_err(|e| (format!("JSON parse error: {}", e), 1))?;

            let compressor = ResponseCompressor::new();
            let result = compressor.compress(&value);
            let output = serde_json::to_string_pretty(&result)
                .map_err(|e| (format!("Serialization error: {}", e), 2))?;
            println!("{}", output);
        }
    }

    Ok(())
}

fn main() {
    if let Err((msg, code)) = run() {
        eprintln!("Error: {}", msg);
        process::exit(code);
    }
}
