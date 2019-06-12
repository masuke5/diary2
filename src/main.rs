#[macro_use]
extern crate serde_derive;

mod page;
mod storage;
mod config;
mod commands;

use std::env;
use std::path::{Path};
use std::io::Read;
use std::fs::File;
use std::fs;
use std::process;
use std::collections::HashMap;
use clap::{Arg, App, SubCommand};
use toml;
use config::Config;

const CONFIG_FILE: &str = "config.toml";

fn load_config(config_file_path: &Path) -> Result<Config, String> {
    if !config_file_path.exists() {
        return Ok(Config::default());
    }

    let mut config_file = File::open(config_file_path).map_err(|err| format!("{}", err))?;
    let mut toml_str = String::new();
    config_file.read_to_string(&mut toml_str).map_err(|err| format!("{}", err))?;

    let config: Config = toml::from_str(&toml_str).map_err(|err| format!("{}", err))?;
    Ok(config)
}

fn main() {
    let directory = Path::new(&env::var("APPDATA").expect("APPDATAが設定されていません")).join("diary2");
    if !directory.exists() {
        fs::create_dir_all(&directory.join(storage::PAGE_DIR))
            .expect(&format!("\"{}\" の作成に失敗しました", directory.join(storage::PAGE_DIR).to_string_lossy()));
    }

    // 設定ファイルを読み込む
    let config_file_path = directory.join(CONFIG_FILE);
    let config = match load_config(&config_file_path) {
        Ok(config) => config,
        Err(err) => {
            println!("設定ファイルの読み込みに失敗しました: {}", err);
            process::exit(1);
        },
    };

    let matches = App::new("diary2")
        .version("1.0")
        .subcommand(SubCommand::with_name("config")
                    .about("edit config")
                    .arg(Arg::with_name("editor")
                         .takes_value(true)
                         .long("editor")
                         .short("e")))
        .subcommand(SubCommand::with_name("list")
                    .alias("ls")
                    .arg(Arg::with_name("limit")
                         .takes_value(true)
                         .long("limit")
                         .short("l")))
        .subcommand(SubCommand::with_name("new")
                    .arg(Arg::with_name("hidden")
                         .long("hidden")
                         .short("d")))
        .subcommand(SubCommand::with_name("lastdt"))
        .subcommand(SubCommand::with_name("show")
                    .arg(Arg::with_name("date")
                         .index(1)))
        .get_matches();

    let mut commands: HashMap<&str, fn(ctx: commands::Context) -> Result<(), failure::Error>> = HashMap::new();
    commands.insert("config", commands::config);
    commands.insert("list", commands::list);
    commands.insert("new", commands::new);
    commands.insert("lastdt", commands::lastdt);
    commands.insert("show", commands::show);

    for (name, func) in commands {
        if let Some(sub_matches) = matches.subcommand_matches(name) {
            if let Err(_) = func(commands::Context::new(&directory, &config_file_path, config, &matches, sub_matches)) {
                std::process::exit(1);
            }
            break;
        }
    }
}
