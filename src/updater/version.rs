use semver::Version;

use super::error::UpdateError;

pub fn is_version_newer(current: &str, other: &str) -> Result<bool, UpdateError> {
    let current = Version::parse(current)?;
    let other = Version::parse(other)?;
    let fork_revision = |version: &Version| {
        let suffix = version.pre.as_str();
        suffix
            .strip_prefix('p')
            .and_then(|revision| revision.parse::<u64>().ok())
            .or_else(|| suffix.strip_prefix("patch.").and_then(|revision| revision.parse::<u64>().ok()))
    };

    if let (Some(current_revision), Some(other_revision)) = (fork_revision(&current), fork_revision(&other)) {
        return Ok((other.major, other.minor, other.patch, other_revision)
            > (current.major, current.minor, current.patch, current_revision));
    }

    Ok(other > current)
}

pub fn is_version_compatible(current: &str, other: &str) -> Result<bool, UpdateError> {
    let current = Version::parse(current)?;
    let other = Version::parse(other)?;

    Ok(if !current.pre.is_empty() {
        current.major == other.major
            && ((other.minor >= current.minor) || (current.minor == other.minor && other.patch >= current.patch))
    } else if other.major == 0 && current.major == 0 {
        current.minor == other.minor && other.patch > current.patch && other.pre.is_empty()
    } else if other.major > 0 {
        current.major == other.major
            && ((other.minor > current.minor) || (current.minor == other.minor && other.patch > current.patch))
            && other.pre.is_empty()
    } else {
        false
    })
}

#[cfg(test)]
mod tests {
    use super::is_version_newer;

    #[test]
    fn fork_patch_updates_follow_numeric_revision_order() {
        for (current, other, expected) in [
            ("1.51.1-patch.1", "1.51.1-p2", true),
            ("1.51.1-p2", "1.51.1-patch.1", false),
            ("1.51.1-p9", "1.51.1-p10", true),
            ("1.51.1-p10", "1.51.1-p9", false),
            ("1.51.1-p2", "1.51.1-p2", false),
            ("1.51.1-p10", "1.52.0-p1", true),
            ("1.52.0-p1", "1.51.1-p10", false),
            ("1.51.1-p2", "1.51.1", true),
            ("1.51.1-alpha.1", "1.51.1-alpha.2", true),
        ] {
            assert_eq!(is_version_newer(current, other).expect("valid version"), expected, "{current} -> {other}");
        }
    }
}
