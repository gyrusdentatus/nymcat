// src/main.rs
mod common;
mod simple;

use common::{Colors, LogLevel, separator};
use std::env;
use std::io::Write;

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
        print_usage(&args[0]);
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
                print_usage(&args[0]);
                return Ok(());
            }
            
            let address = args[2].clone();
            let username = args[3].clone();
            
            simple::run_chat_client(username, address, verbosity, env_file).await?;
        },
        
        _ => {
            print_usage(&args[0]);
        }
    }
    
    Ok(())
}

fn print_usage(program_name: &str) {
    println!("\n{}", separator(Some("Usage"), 80));
    println!("{}Error:{} Invalid command or arguments\n", Colors::RED, Colors::RESET);
    
    println!("{}Create a chat room:{}", Colors::BRIGHT_YELLOW, Colors::RESET);
    println!("    {} create [--env <env_file>] [-v|-vv|-vvv]", program_name);
    
    println!("\n{}Join a chat room:{}", Colors::BRIGHT_YELLOW, Colors::RESET);
    println!("    {} join <address> <username> [--env <env_file>] [-v|-vv|-vvv]", program_name);
    
    println!("\n{}Verbosity levels:{}", Colors::BRIGHT_YELLOW, Colors::RESET);
    println!("    -v    Info messages");
    println!("    -vv   Debug messages");
    println!("    -vvv  Trace messages (detailed)");
    
    println!("\n{}Additional options:{}", Colors::BRIGHT_YELLOW, Colors::RESET);
    println!("    --env <file>  Specify Nym network environment file");
    
    println!("{}\n", separator(None, 80));
}
