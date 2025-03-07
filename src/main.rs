// src/main.rs
mod common;
mod simple;

use common::LogLevel;
use std::env;

fn get_verbosity(args: &[String]) -> LogLevel {
    for arg in args {
        match arg.as_str() {
            "-v" => return LogLevel::Info,
            "-vv" => return LogLevel::Debug,
            "-vvv" => return LogLevel::Trace,
            _ => {}
        }
    }
    LogLevel::None
}

fn get_env_file(args: &[String]) -> Option<String> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--env" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        eprintln!("Usage: 
    {} create [--env <env_file>] [-v|-vv|-vvv]
    {} join <address> <username> [--env <env_file>] [-v|-vv|-vvv]", args[0], args[0]);
        return Ok(());
    }

    let verbosity = get_verbosity(&args);
    let env_file = get_env_file(&args);

    match args[1].as_str() {
        "create" => {
            simple::run_room_server(verbosity, env_file).await?;
        },
        
        "join" => {
            if args.len() < 4 {
                eprintln!("Usage: {} join <address> <username> [--env <env_file>] [-v|-vv|-vvv]", args[0]);
                return Ok(());
            }
            
            let address = args[2].clone();
            let username = args[3].clone();
            
            simple::run_chat_client(username, address, verbosity, env_file).await?;
        },
        
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            eprintln!("Usage: 
    {} create [--env <env_file>] [-v|-vv|-vvv]
    {} join <address> <username> [--env <env_file>] [-v|-vv|-vvv]", args[0], args[0]);
        }
    }
    
    Ok(())
}
