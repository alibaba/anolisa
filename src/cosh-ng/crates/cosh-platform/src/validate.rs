//! Input validation for CLI arguments.

use cosh_types::error::{CoshError, ErrorCode};

/// Validate a package name. Allows `[a-zA-Z0-9._+:-]`.
pub fn validate_pkg_name(name: &str) -> Result<(), CoshError> {
    if name.is_empty() {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            "Package name cannot be empty",
            "pkg",
        ));
    }
    if name.len() > 256 {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            "Package name too long (max 256 characters)",
            "pkg",
        ));
    }
    if let Some(c) = name.chars().find(|c| !is_valid_pkg_char(*c)) {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            format!("Invalid character '{}' in package name '{}'", c, name),
            "pkg",
        )
        .with_hint("Package names may only contain: a-z A-Z 0-9 . _ + - :"));
    }
    Ok(())
}

/// Validate a service name. Allows `[a-zA-Z0-9._@:-]`.
pub fn validate_svc_name(name: &str) -> Result<(), CoshError> {
    if name.is_empty() {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            "Service name cannot be empty",
            "svc",
        ));
    }
    if name.len() > 256 {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            "Service name too long (max 256 characters)",
            "svc",
        ));
    }
    if let Some(c) = name.chars().find(|c| !is_valid_svc_char(*c)) {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            format!("Invalid character '{}' in service name '{}'", c, name),
            "svc",
        )
        .with_hint("Service names may only contain: a-z A-Z 0-9 . _ @ - :"));
    }
    Ok(())
}

fn is_valid_pkg_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | ':')
}

fn is_valid_svc_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '@' | '-' | ':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_pkg_names() {
        assert!(validate_pkg_name("nginx").is_ok());
        assert!(validate_pkg_name("nginx-1.24.0").is_ok());
        assert!(validate_pkg_name("lib_ssl3").is_ok());
        assert!(validate_pkg_name("gcc-c++").is_ok());
        assert!(validate_pkg_name("perl:5.38").is_ok());
    }

    #[test]
    fn test_invalid_pkg_names() {
        assert!(validate_pkg_name("").is_err());
        assert!(validate_pkg_name("nginx;rm -rf /").is_err());
        assert!(validate_pkg_name("pkg|cat /etc/passwd").is_err());
        assert!(validate_pkg_name("pkg&bg").is_err());
        assert!(validate_pkg_name("pkg$VAR").is_err());
        assert!(validate_pkg_name("pkg`cmd`").is_err());
        assert!(validate_pkg_name("pkg\nnewline").is_err());
        assert!(validate_pkg_name("pkg name").is_err());
    }

    #[test]
    fn test_pkg_name_too_long() {
        let long_name = "a".repeat(257);
        let err = validate_pkg_name(&long_name).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn test_valid_svc_names() {
        assert!(validate_svc_name("nginx").is_ok());
        assert!(validate_svc_name("nginx.service").is_ok());
        assert!(validate_svc_name("user@1000").is_ok());
        assert!(validate_svc_name("dbus-org.freedesktop.timesync1").is_ok());
    }

    #[test]
    fn test_invalid_svc_names() {
        assert!(validate_svc_name("").is_err());
        assert!(validate_svc_name("svc;rm").is_err());
        assert!(validate_svc_name("svc|cat").is_err());
        assert!(validate_svc_name("svc$VAR").is_err());
    }
}
