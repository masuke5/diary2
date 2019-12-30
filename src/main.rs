#[macro_use]
extern crate serde_derive;

mod page;
mod storage;
mod config;
mod commands;
mod dropbox;
mod secret;

use std::env;
use std::path::{Path, PathBuf};
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

fn get_directory() -> PathBuf {
    if cfg!(windows) {
        Path::new(&env::var("APPDATA").expect("APPDATAが設定されていません")).join("diary2")
    } else {
        let config_dir = env::var("XDG_CONFIG_HOME")
            .map(|dir| Path::new(&dir).to_path_buf())
            .unwrap_or(Path::new(&env::var("HOME").expect("HOMEが設定されていません")).join(".config"));
        config_dir.join("diary2")
    }
}

fn main() {
    let directory = get_directory();
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
        .subcommand(SubCommand::with_name("search")
                    .arg(Arg::with_name("query")
                         .index(1))
                    .arg(Arg::with_name("title")
                         .long("title")
                         .short("t"))
                    .arg(Arg::with_name("text")
                         .long("text")
                         .short("b"))
                    .arg(Arg::with_name("show-first")
                         .long("show-first")
                         .short("f"))
                    .arg(Arg::with_name("limit")
                         .takes_value(true)
                         .long("limit")
                         .short("l")))
        .subcommand(SubCommand::with_name("amend"))
        .subcommand(SubCommand::with_name("auth"))
        .subcommand(SubCommand::with_name("sync"))
        .get_matches();

    let mut commands: HashMap<&str, fn(ctx: commands::Context) -> Result<(), failure::Error>> = HashMap::new();
    commands.insert("config", commands::config);
    commands.insert("list", commands::list);
    commands.insert("new", commands::new);
    commands.insert("lastdt", commands::lastdt);
    commands.insert("show", commands::show);
    commands.insert("amend", commands::amend);
    commands.insert("search", commands::search);
    commands.insert("auth", commands::auth);
    commands.insert("sync", commands::sync);

    for (name, func) in commands {
        if let Some(sub_matches) = matches.subcommand_matches(name) {
            if let Err(_) = func(commands::Context::new(&directory, &config_file_path, config, &matches, sub_matches)) {
                std::process::exit(1);
            }
            break;
        }
    }
}
