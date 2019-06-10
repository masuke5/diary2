use std::path::Path;
use std::fs::File;
use crate::page::{Page, WeekPage};
use chrono::{Date, Utc, Weekday, Datelike};
use failure;

fn find_this_week() -> (Date<Utc>, Date<Utc>) {
    let today = Utc::today();
    let mut begin = today;
    let mut end = today;
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

pub fn list<P: AsRef<Path>>(directory: P) -> Result<Vec<Page>, failure::Error> {
    let (week_begin, week_end) = find_this_week();
    let filename = format!("{}-{}.json", week_begin.format("%Y-%m-%d"), week_end.format("%Y-%m-%d"));
    let mut file = File::open(directory.join(&filename))?; 
    let week_page: WeekPage = serde_json::from_reader(file)?;
}
