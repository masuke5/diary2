#[macro_use]
extern crate serde_derive;

mod commands;
mod config;
mod dropbox;
mod page;
mod secret;
mod storage;

use std::env;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result};
use clap::{App, Arg, SubCommand};
use tokio::fs;
use tokio::io;

use config::Config;

const CONFIG_FILE: &str = "config.toml";
const PAGE_VERSION_FILE: &str = "page_version";

async fn load_config(config_file_path: &Path) -> Result<Config> {
    if !config_file_path.exists() {
        return Ok(Config::default());
    }

    let toml_str = fs::read_to_string(config_file_path).await?;
    let config: Config = toml::from_str(&toml_str)?;

    Ok(config)
}

fn get_directory() -> PathBuf {
    if cfg!(windows) {
        Path::new(&env::var("APPDATA").expect("APPDATAが設定されていません")).join("diary2")
    } else {
        let config_dir = env::var("XDG_CONFIG_HOME")
            .map(|dir| Path::new(&dir).to_path_buf())
            .unwrap_or_else(|_| {
                Path::new(&env::var("HOME").expect("HOMEが設定されていません")).join(".config")
            });
        config_dir.join("diary2")
    }
}

async fn get_page_version(base: &Path) -> u32 {
    let file_path = base.join(PAGE_VERSION_FILE);

    if file_path.exists() {
        let version = fs::read_to_string(file_path)
            .await
            .expect("バージョンの読み込みに失敗しました");
        version.parse().expect("バージョンが数字ではありません")
    } else {
        fs::write(file_path, &format!("{}", page::CURRENT_PAGE_VERSION))
            .await
            .expect("バージョンの書き込みに失敗しました");
        page::CURRENT_PAGE_VERSION
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let directory = get_directory();

    // 必要なディレクトリを作成する
    fs::create_dir_all(&directory.join(storage::PAGE_DIR))
        .await
        .with_context(|| {
            format!(
                "\"{}\" の作成に失敗しました",
                directory.join(storage::PAGE_DIR).display(),
            )
        })?;

    fs::create_dir_all(&directory.join(storage::IMAGE_DIR))
        .await
        .with_context(|| {
            format!(
                "\"{}\" の作成に失敗しました",
                directory.join(storage::PAGE_DIR).to_string_lossy()
            )
        })?;

    let page_version = get_page_version(&directory).await;

    let matches = App::new("diary2")
        .version("1.2.0")
        .subcommand(
            SubCommand::with_name("config").about("edit config").arg(
                Arg::with_name("editor")
                    .takes_value(true)
                    .long("editor")
                    .short("e"),
            ),
        )
        .subcommand(
            SubCommand::with_name("list").alias("ls").arg(
                Arg::with_name("limit")
                    .takes_value(true)
                    .long("limit")
                    .short("l"),
            ),
        )
        .subcommand(
            SubCommand::with_name("new").arg(Arg::with_name("hidden").long("hidden").short("d")),
        )
        .subcommand(SubCommand::with_name("lastdt"))
        .subcommand(
            SubCommand::with_name("show")
                .arg(Arg::with_name("date").index(1))
                .arg(Arg::with_name("stdout").long("stdout").short("s")),
        )
        .subcommand(
            SubCommand::with_name("search")
                .arg(Arg::with_name("query").index(1))
                .arg(Arg::with_name("title").long("title").short("t"))
                .arg(Arg::with_name("text").long("text").short("b"))
                .arg(Arg::with_name("show-first").long("show-first").short("f"))
                .arg(Arg::with_name("stdout").long("stdout").short("s"))
                .arg(
                    Arg::with_name("limit")
                        .takes_value(true)
                        .long("limit")
                        .short("l"),
                ),
        )
        .subcommand(SubCommand::with_name("amend"))
        .subcommand(SubCommand::with_name("auth"))
        .subcommand(SubCommand::with_name("sync"))
        .subcommand(SubCommand::with_name("fixpage"))
        .get_matches();

    // 設定ファイルを読み込む
    let config_file_path = directory.join(CONFIG_FILE);
    let config = match load_config(&config_file_path).await {
        Ok(config) => config,
        Err(err) => {
            if matches.subcommand_matches("config").is_some() {
                eprintln!("設定ファイルの読み込みに失敗したため、デフォルトの設定で続行します。");
                eprintln!("詳細: {}", err);
                Config::default()
            } else {
                if let Some(err) = err.downcast_ref::<io::Error>() {
                    eprintln!("設定ファイルの読み込みに失敗しました: {}", err);
                } else if let Some(err) = err.downcast_ref::<toml::de::Error>() {
                    eprintln!("設定ファイルのパースに失敗しました: {}", err);
                    eprintln!("`diary2 config` を実行して設定を修正してください。");
                } else {
                    eprintln!("設定の読み込みに失敗しました: {}", err);
                }

                process::exit(1);
            }
        }
    };

    let (name, sub_matches) = matches.subcommand();
    let sub_matches = sub_matches.unwrap();

    if name != "fixpage" && page_version != page::CURRENT_PAGE_VERSION {
        eprintln!("ページの保存形式が違います。");
        eprintln!("`diary2 fixpage` を実行してください。");
        process::exit(2);
    }

    let ctx = commands::Context::new(
        &directory,
        &config_file_path,
        config,
        &matches,
        sub_matches,
        page_version,
    );

    let result = match name {
        "config" => commands::config(ctx),
        "list" => commands::list(ctx).await,
        "new" => commands::new(ctx).await,
        "lastdt" => commands::lastdt(ctx).await,
        "show" => commands::show(ctx).await,
        "amend" => commands::amend(ctx).await,
        "search" => commands::search(ctx).await,
        "auth" => commands::auth(ctx).await,
        "sync" => commands::sync(ctx).await,
        "fixpage" => commands::fixpage(ctx).await,
        _ => panic!(),
    };

    if let Err(err) = result {
        eprintln!("{}", err);
        for cause in err.chain().skip(1) {
            eprintln!("詳細: {}", cause);
        }

        std::process::exit(1);
    }

    if name == "fixpage" {
        let file_path = directory.join(PAGE_VERSION_FILE);
        fs::write(&file_path, &format!("{}", page::CURRENT_PAGE_VERSION))
            .await
            .unwrap();
    }

    Ok(())
}
