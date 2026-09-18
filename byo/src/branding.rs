//! Whose journey this is.
//!
//! The owner's name is personal, so it is never compiled in: it comes from
//! `PUBLIC_JOURNEY_OWNER` (the repo-root `.env`, which `install.sh` sources and the site
//! bakes into its build). With nothing set everything reads "The Journey".

/// Possessive owner label: `Ada's` when configured, `The` otherwise.
pub fn owner_label() -> String {
    match std::env::var("PUBLIC_JOURNEY_OWNER") {
        Ok(v) if !v.trim().is_empty() => format!("{}'s", v.trim()),
        _ => "The".to_string(),
    }
}

/// Title shown by `byo status` and the "site not built" placeholder page.
pub fn title() -> String {
    format!("{} Journey", owner_label())
}

#[cfg(test)]
mod tests {
    #[test]
    fn falls_back_to_an_anonymous_title() {
        // The var is process-wide, so assert on the helper's logic through both branches.
        let restore = std::env::var("PUBLIC_JOURNEY_OWNER").ok();
        std::env::remove_var("PUBLIC_JOURNEY_OWNER");
        assert_eq!(super::title(), "The Journey");
        std::env::set_var("PUBLIC_JOURNEY_OWNER", "  Ada  ");
        assert_eq!(super::title(), "Ada's Journey");
        std::env::set_var("PUBLIC_JOURNEY_OWNER", "   ");
        assert_eq!(super::title(), "The Journey");
        match restore {
            Some(v) => std::env::set_var("PUBLIC_JOURNEY_OWNER", v),
            None => std::env::remove_var("PUBLIC_JOURNEY_OWNER"),
        }
    }
}
