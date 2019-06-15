use std::path::{Path, PathBuf};
use std::fs::{File, DirEntry};
use std::cmp::Reverse;
use crate::page::{Page, WeekPage};
use chrono::{Date, Utc, Weekday, Datelike};
use std::mem;
use failure;

pub const PAGE_DIR: &str = "pages";

// 日曜日と土曜日の日付を取得
fn find_week(day: &Date<Utc>) -> (Date<Utc>, Date<Utc>) {
    let mut begin = day.clone();
    let mut end = day.clone();
    loop {
        match begin.weekday() {
            Weekday::Sun => break,
            _ => begin = begin.pred(),
        };
    }

    loop {
        match end.weekday() {
            Weekday::Sat => break,
            _ => end = end.succ(),
        };
    }

    (begin, end)
}

fn generate_page_filepath(directory: &Path, date: &Date<Utc>) -> PathBuf {
    let (week_begin, week_end) = find_week(date);
    let filename = format!("{}-{}.json", week_begin.format("%Y-%m-%d"), week_end.format("%Y-%m-%d"));
    let filepath = directory.join(PAGE_DIR).join(&filename);
    filepath
}

pub fn write(directory: &Path, page: Page) -> Result<(), failure::Error> {
    let filepath = generate_page_filepath(directory, &Utc::today());

    // なぜか追記される
    // let file = OpenOptions::new()
    //     .read(true)
    //     .write(true)
    //     .create(true)
    //     .open(&filepath)?;

    let exists = filepath.exists();
    let mut week_page = if exists {
        // ファイルが存在したら今週のページを読み込む
        let file = File::open(&filepath)?;
        serde_json::from_reader(file)?
    } else {
        // ファイルが存在しなかったらWeekPageを作成
        WeekPage::new()
    };

    match week_page.pages.iter().position(|old_page| page.created_at == old_page.created_at) {
         // ページが存在したら更新 
        Some(pos) => { mem::replace(&mut week_page.pages[pos], page); },
        // 存在しなかったら追加する
        None => week_page.pages.push(page),
    };

    let file = File::create(&filepath)?; 
    serde_json::to_writer(file, &week_page)?;

    Ok(())
}

pub fn list(directory: &Path, limit: u32) -> Result<Vec<Page>, failure::Error> {
    // ページが格納されているディレクトリのファイルをすべて取得する
    let mut entries: Vec<DirEntry> = directory.join(PAGE_DIR)
        .read_dir()?
        .filter(|entry| entry.is_ok())
        .map(|entry| entry.unwrap()).collect();
    // ファイル名で降順にソート
    entries.sort_by_key(|entry| Reverse(entry.file_name()));

    let mut pages: Vec<Page> = Vec::new();
    let mut count = 0u32;

    'a: for entry in entries {
        let file = File::open(entry.path())?;

        let mut week_page: WeekPage = serde_json::from_reader(file)?;
        week_page.pages.sort_by_key(|page| Reverse(page.created_at));
        for page in week_page.pages {
            if count >= limit {
                break 'a;
            }
            pages.push(page);
            count += 1;
        }
    }

    Ok(pages)
}

pub fn get_week_page(directory: &Path, date: &Date<Utc>) -> Result<Option<WeekPage>, failure::Error> {
    let filepath = generate_page_filepath(directory, date);
    if !filepath.exists() {
        return Ok(None);
    }

    let file = File::open(filepath)?;

    let week_page: WeekPage = serde_json::from_reader(file)?;
    Ok(Some(week_page))
}
