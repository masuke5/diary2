use std::path::Path;
use std::fs::File;
use std::io::BufReader;
use crate::page::{Page, WeekPage};
use chrono::{Date, Utc, Weekday, Datelike};
use failure;

pub const PAGE_DIR: &str = "pages";

// 日曜日と土曜日の日付を取得
fn find_week(day: Date<Utc>) -> (Date<Utc>, Date<Utc>) {
    let mut begin = day;
    let mut end = day;
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

pub fn write(directory: &Path, page: Page) -> Result<(), failure::Error> {
    // 今週の日曜日と土曜日の日付からファイルパスを生成
    let (week_begin, week_end) = find_week(Utc::today());
    let filename = format!("{}-{}.json", week_begin.format("%Y-%m-%d"), week_end.format("%Y-%m-%d"));
    let filepath = directory.join(PAGE_DIR).join(&filename);

    let exists = filepath.exists();
    let mut week_page = if exists {
        // ファイルが存在したら今週のページを読み込む
        let file = File::open(&filepath)?;
        serde_json::from_reader(file)?
    } else {
        // ファイルが存在しなかったらWeekPageを作成
        WeekPage::new()
    };

    // 追加して書き込む
    week_page.pages.push(page);

    let file = File::create(&filepath)?; 
    serde_json::to_writer(file, &week_page)?;

    Ok(())
}
