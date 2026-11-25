//! The `.ibcmap` text as text: header lines and the character grid, before the engine interprets it.
//! Lint needs this to report problems the engine parser rejects or silently accepts, and the
//! transforms rewrite it.

/// Characters the engine parser accepts in the grid.
pub fn is_tile_char(c: u8) -> bool {
    matches!(c, b'#' | b'.' | b'o' | b'a' | b'd' | b'1'..=b'9')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMap {
    /// Header lines before `---`, verbatim (blank lines dropped).
    pub header: Vec<String>,
    /// Grid rows, trailing whitespace trimmed, blank rows dropped (as the engine does).
    pub rows: Vec<Vec<u8>>,
}

impl RawMap {
    pub fn parse(text: &str) -> Result<RawMap, String> {
        let mut header = Vec::new();
        let mut lines = text.lines();
        let mut found = false;
        for line in lines.by_ref() {
            let line = line.trim_end();
            if line == "---" {
                found = true;
                break;
            }
            if !line.is_empty() {
                header.push(line.to_string());
            }
        }
        if !found {
            return Err("missing `---` line between the header and the grid".into());
        }
        let rows: Vec<Vec<u8>> = lines
            .map(|l| l.trim_end())
            .filter(|l| !l.is_empty())
            .map(|l| l.as_bytes().to_vec())
            .collect();
        if rows.is_empty() {
            return Err("map has no grid rows".into());
        }
        Ok(RawMap { header, rows })
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for h in &self.header {
            out.push_str(h);
            out.push('\n');
        }
        out.push_str("---\n");
        for r in &self.rows {
            out.push_str(&String::from_utf8_lossy(r));
            out.push('\n');
        }
        out
    }

    /// Value of a `key: value` header line.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .filter(|l| !l.starts_with("//"))
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                (k.trim() == key).then_some(v.trim())
            })
    }

    /// Replace or append a `key: value` header line.
    pub fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}: {value}");
        let at = self.header.iter().position(|l| {
            !l.starts_with("//") && l.split_once(':').is_some_and(|(k, _)| k.trim() == key)
        });
        match at {
            Some(i) => self.header[i] = line,
            None => self.header.push(line),
        }
    }

    pub fn width(&self) -> usize {
        self.rows.first().map_or(0, |r| r.len())
    }

    pub fn height(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_edits_header() {
        let text = "name: t\n// note\nteam_size: 3\n---\n###\n#a#\n###\n";
        let mut raw = RawMap::parse(text).unwrap();
        assert_eq!(raw.to_text(), text);
        assert_eq!(
            (raw.get("name"), raw.get("team_size"), raw.get("nope")),
            (Some("t"), Some("3"), None)
        );
        raw.set("name", "u");
        raw.set("author", "me");
        assert_eq!(
            raw.header,
            vec!["name: u", "// note", "team_size: 3", "author: me"]
        );
        assert!(RawMap::parse("name: t\n###\n").is_err());
    }
}
