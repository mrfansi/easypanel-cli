use regex::RegexBuilder;

/// Client-side list filter used by both EasyPanel and Cloudflare TUI screens.
///
/// A filter keeps the old case-insensitive substring behavior, and additionally
/// treats a valid regex as another way to match. Invalid regex input is never an
/// error while typing; it simply falls back to substring matching until the
/// pattern becomes valid.
pub struct FilterMatcher<'a> {
    raw: &'a str,
    lower: String,
    regex: Option<regex::Regex>,
}

impl<'a> FilterMatcher<'a> {
    pub fn new(raw: &'a str) -> Self {
        Self {
            raw,
            lower: raw.to_ascii_lowercase(),
            regex: if raw.is_empty() {
                None
            } else {
                RegexBuilder::new(raw).case_insensitive(true).build().ok()
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    pub fn matches(&self, cell: &str) -> bool {
        self.is_empty()
            || cell.to_ascii_lowercase().contains(&self.lower)
            || self.regex.as_ref().is_some_and(|re| re.is_match(cell))
    }

    pub fn matches_any<'b, I>(&self, cells: I) -> bool
    where
        I: IntoIterator<Item = &'b str>,
    {
        self.is_empty() || cells.into_iter().any(|cell| self.matches(cell))
    }
}
