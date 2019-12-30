use chrono::{DateTime, Utc};

pub const CURRENT_PAGE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageV1 {
    pub title: String,
    pub text: String,
    pub hidden: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: Vec<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub text: String,
    pub hidden: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: Vec<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeekPageV1 {
    pub pages: Vec<PageV1>,
    pub uploaded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeekPage {
    pub pages: Vec<Page>,
    pub uploaded_at: Option<DateTime<Utc>>,
}

impl WeekPage {
    pub fn new() -> Self {
        Self {
            pages: Vec::new(),
            uploaded_at: None,
        }
    }
}
