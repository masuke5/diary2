#[macro_use]
extern crate serde_derive;

mod page;
mod storage;
mod config;

use std::env;
use std::path::{Path};
use std::io::Read;
use std::fs::File;
use std::fs;
use std::process;
use chrono;
use clap::{Arg, App, SubCommand};
use toml;
use config::Config;

const CONFIG_FILE: &str = "config.toml";

fn load_config(directory: &Path) -> Result<Config, String> {
    let config_file_path = directory.join(CONFIG_FILE);
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
    let config = match load_config(&directory) {
        Ok(config) => config,
        Err(err) => {
            println!("設定ファイルの読み込みに失敗しました: {}", err);
            process::exit(1);
        },
    };

    println!("{:?}", config);
}
