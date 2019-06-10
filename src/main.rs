#[macro_use]
extern crate serde_derive;

use chrono;
use clap::{Arg, App, SubCommand};
use std::env;
use std::path::{Path};
use std::fs;

mod page;
mod storage;

fn main() {
    let directory = Path::new(&env::var("APPDATA").expect("APPDATAが設定されていません")).join("diary2");
    if !directory.exists() {
        fs::create_dir_all(&directory.join(storage::PAGE_DIR))
            .expect(&format!("\"{}\" の作成に失敗しました", directory.join(storage::PAGE_DIR).to_string_lossy()));
    }

    let page = page::Page {
        title: String::from("test"),
        text: String::from("テストページ"),
        hidden: false,
        created_at: chrono::Utc::now(),
        updated_at: vec![chrono::Utc::now()],
    };
    storage::write(&directory, page).expect("FAIL");
}
