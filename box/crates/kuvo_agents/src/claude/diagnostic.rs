//! Keep complete diagnostic lines from the current process and remove credentials before display.

use regex::{Captures, Regex};

const LIMIT: usize = 4000;

#[derive(Default)]
pub(super) struct Said {
    partial: String,
    last: Option<String>,
    cut: bool,
}

impl Said {
    pub fn add(&mut self, text: &str) {
        for line in text.split_inclusive('\n') {
            if self.partial.len() + line.len() > LIMIT {
                self.partial.clear();
                self.cut = true;
            }
            if !self.cut {
                self.partial.push_str(line);
            }
            if line.ends_with('\n') {
                self.finish_line();
            }
        }
    }

    fn finish_line(&mut self) {
        let line = self.partial.trim();
        if !self.cut
            && !line.is_empty()
            && !["at ", "Bun v", "Node.js v"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        {
            self.last = Some(line.to_owned());
        }
        self.partial.clear();
        self.cut = false;
    }

    pub fn forget(&mut self) {
        self.cut |= !self.partial.is_empty();
        self.partial.clear();
        self.last = None;
    }

    pub fn last(&mut self, exited: bool) -> Option<String> {
        if exited {
            self.finish_line();
        }
        self.last
            .as_deref()
            .map(|line| redact(line).chars().take(300).collect())
    }
}

pub(super) fn redact(text: &str) -> String {
    const VALUE: &str = r#""[^"]*"|'[^']*'|[^\s"',;}]+"#;
    const NAME: &str = r"[\w-]*(?:key|token|secret|password|passwd)[\w-]*";
    let mut filtered = text.to_owned();
    for (pattern, replacement) in [
        (r"sk-ant-[A-Za-z0-9_-]+", "sk-ant-…"),
        (r"://[^\s/]*@", "://…@"),
        (
            r#"(?i)\b((?:proxy-)?authorization["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|(?:(?:bearer|basic|token)\s+)?[^\s"',;]+)"#,
            "${1}…",
        ),
    ] {
        filtered = Regex::new(pattern)
            .expect("credential pattern")
            .replace_all(&filtered, replacement)
            .into_owned();
    }
    filtered = Regex::new(r"(?i)\b(Bearer\s+)([A-Za-z0-9._~+/-]+=*)")
        .expect("bearer pattern")
        .replace_all(&filtered, |m: &Captures<'_>| {
            if m[2].eq_ignore_ascii_case("token") || m[2].eq_ignore_ascii_case("tokens") {
                m[0].to_owned()
            } else {
                format!("{}…", &m[1])
            }
        })
        .into_owned();
    let pattern = format!(
        r#"(?i)\b({NAME})(\s*=\s*)(?:{VALUE})|(["'])({NAME})(["']\s*:\s*)(?:{VALUE})|(--{NAME})(\s+|=)(?:{VALUE})|\b([\w-]*(?:password|passwd|secret)|[\w-]+[-_](?:api[-_]?key|token))(\s*:\s*)(?:{VALUE})"#
    );
    Regex::new(&pattern)
        .expect("named credential pattern")
        .replace_all(&filtered, |m: &Captures<'_>| {
            if let Some(name) = m.get(1) {
                if name.as_str().to_ascii_lowercase().ends_with("tokens") {
                    return m[0].to_owned();
                }
                return format!("{}{}…", &m[1], &m[2]);
            }
            if m.get(3).is_some() {
                return format!("{}{}{}…", &m[3], &m[4], &m[5]);
            }
            if m.get(6).is_some() {
                return format!("{}{}…", &m[6], &m[7]);
            }
            format!("{}{}…", &m[8], &m[9])
        })
        .into_owned()
}
