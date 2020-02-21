use std::borrow::Cow;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use regex::{Captures, Regex};

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

pub fn convert_image_paths_in_text<'a, F>(
    text: &'a str,
    mut f: F,
) -> (Cow<'a, str>, Vec<(PathBuf, String)>)
where
    F: FnMut(&str) -> String,
{
    let mut images = Vec::new();

    let re = Regex::new(r#"!\[(.*?)\]\((.*?)\)"#).unwrap();
    let result = re.replace_all(text, |cap: &Captures| {
        let file_name = match PathBuf::from(&cap[2]).file_name() {
            Some(file_name) => file_name.to_string_lossy().to_string(),
            None => return cap[0].into(),
        };

        let converted = f(&file_name);
        images.push((PathBuf::from(&file_name), converted.clone()));
        format!("![{}]({})", &cap[1], converted)
    });

    (result, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_image_paths_in_text() {
        let text = r#"# Title
aa![abcd](/home/aa/image.png)ab![aa]()jfewa
f![efgh](image2.jpeg)a"#;
        let expected = r#"# Title
aa![abcd](prefix-image.png)ab![aa]()jfewa
f![efgh](prefix-image2.jpeg)a"#;

        let (converted, images) = convert_image_paths_in_text(text, |s| format!("prefix-{}", s));

        assert_eq!(expected, converted);
        assert_eq!(
            vec![
                (PathBuf::from("image.png"), String::from("prefix-image.png")),
                (PathBuf::from("image2.jpeg"), String::from("prefix-image2.jpeg")),
            ],
            images
        );
    }
}
