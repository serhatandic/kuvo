//! Display units and storage budget confirmation wording matching `shared/artifact-budget.ts`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSizeUnit {
    pub name: &'static str,
    pub factor: u64,
    pub decimals: u8,
    pub rounding: &'static str,
}

const UNIT_NAMES: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];

pub fn file_size_unit(bytes: u64) -> FileSizeUnit {
    let mut scale: usize = if bytes > 0 {
        (((bytes as f64).log10() / 3.0).floor() as isize).clamp(0, 4) as usize
    } else {
        0
    };
    let factor = 1000u64.pow(scale as u32);
    if scale < 4 && (((bytes as f64) / (factor as f64) * 10.0).round() / 10.0) >= 1000.0 {
        scale += 1;
    }
    FileSizeUnit {
        name: UNIT_NAMES[scale],
        factor: 1000u64.pow(scale as u32),
        decimals: 1,
        rounding: "nearest",
    }
}

pub fn file_size(bytes: u64) -> String {
    let unit = file_size_unit(bytes);
    let tenths = ((bytes as f64) / (unit.factor as f64) * 10.0).round() as u64;
    let whole = tenths / 10;
    let frac = tenths % 10;
    let formatted_whole = format_with_commas(whole);
    if frac == 0 {
        format!("{formatted_whole} {}", unit.name)
    } else {
        format!("{formatted_whole}.{frac} {}", unit.name)
    }
}

fn format_with_commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (digits.len() - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub struct BudgetConfirmation {
    pub title: &'static str,
    pub body: String,
    pub button: &'static str,
}

pub fn budget_confirmation(older: u64) -> BudgetConfirmation {
    BudgetConfirmation {
        title: "Keep less of older versions?",
        body: format!(
            "Older versions take {} now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.",
            file_size(older)
        ),
        button: "Keep up to",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_that_rounds_up_to_a_thousand_uses_next_unit() {
        assert_eq!(file_size(999_949), "999.9 KB");
        assert_eq!(file_size(999_999), "1 MB");
        assert_eq!(file_size(999_999_999), "1 GB");
        assert_eq!(file_size(999), "999 bytes");
        assert_eq!(file_size(1_500_000), "1.5 MB");
        let unit = file_size_unit(999_999);
        assert_eq!(unit.name, "MB");
        assert_eq!(unit.factor, 1_000_000);
    }
}
