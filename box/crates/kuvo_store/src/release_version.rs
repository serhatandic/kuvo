//! A Kuvo release version, with betas ordered before the final release.

fn parts(version: &str) -> Option<([&str; 3], Option<&str>)> {
    let (base, beta) = version
        .split_once("-beta.")
        .map_or((version, None), |(base, beta)| (base, Some(beta)));
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let mut components = base.split('.');
    let numbers = [components.next()?, components.next()?, components.next()?];
    (components.next().is_none()
        && numbers.iter().all(|part| digits(part))
        && beta.is_none_or(digits))
    .then_some((numbers, beta))
}

pub fn valid(version: &str) -> bool {
    parts(version).is_some()
}

/// Signed releases use bounded integer components.
pub fn order(version: &str) -> Option<([u64; 3], bool, u64)> {
    let ([major, minor, patch], beta) = parts(version)?;
    Some((
        [
            major.parse().ok()?,
            minor.parse().ok()?,
            patch.parse().ok()?,
        ],
        beta.is_none(),
        beta.map(str::parse).transpose().ok()?.unwrap_or(0),
    ))
}

/// Desktop registration keeps the predecessor's decimal version syntax, including
/// arbitrarily large components. Its comparison used JSON's number semantics.
pub fn newer(version: &str, than: &str) -> bool {
    fn numbers(version: &str) -> Option<[f64; 4]> {
        let ([major, minor, patch], beta) = parts(version)?;
        Some([
            major.parse().ok()?,
            minor.parse().ok()?,
            patch.parse().ok()?,
            beta.map(str::parse)
                .transpose()
                .ok()?
                .unwrap_or(f64::INFINITY),
        ])
    }
    numbers(version).is_some_and(|a| numbers(than).is_none_or(|b| a > b))
}
