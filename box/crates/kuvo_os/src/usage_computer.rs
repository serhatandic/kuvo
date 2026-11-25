//! The stable computer label used by historical usage records. It is hashed before
//! it is stored in or shared with Insights.
pub fn identity(hostname: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            && let Ok(text) = String::from_utf8(output.stdout)
        {
            for line in text.lines() {
                if let Some((_, value)) = line.split_once("\"IOPlatformUUID\"")
                    && let Some((_, value)) = value.split_once('=')
                {
                    let uuid = value.trim().trim_matches('"');
                    if uuid.len() >= 16 && uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                    {
                        return format!("mac:{uuid}");
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(id) = std::fs::read_to_string(path) {
                let id = id.trim();
                if id.len() >= 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return format!("machine-id:{}\n{hostname}", id.to_ascii_lowercase());
                }
            }
        }
    }
    format!("host:{hostname}")
}
