use chrono::{DateTime, Utc};

#[derive(Debug, Serialize, Deserialize)]
pub struct Page {
    pub title: String,
    pub text: String,
    pub hidden: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: Vec<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WeekPage {
    pub pages: Vec<Page>,
}
