#[derive(Debug, Deserialize)]
pub struct Config {
    pub editor: String,
    pub default_list_limit: u32,
}

impl Config {
    pub fn default() -> Self {
        Self {
            editor: String::from("vim"),
            default_list_limit: 7,
        }
    }
}