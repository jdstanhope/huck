//! Job-spec parser. Job specs are runtime-only — the lexer/parser
//! doesn't know about them. Builtins call `parse_job_spec` on any
//! argument starting with `%`.

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum JobSpec {
    Id(u32),
    Current,
    Previous,
    Prefix(String),
    Substring(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum JobSpecError {
    Empty,
    BadNumber,
    BadSymbol,
}

pub fn parse_job_spec(s: &str) -> Result<JobSpec, JobSpecError> {
    let rest = match s.strip_prefix('%') {
        Some(r) => r,
        None => return Err(JobSpecError::BadSymbol),
    };
    if rest.is_empty() {
        return Err(JobSpecError::Empty);
    }
    match rest {
        "+" | "%" => return Ok(JobSpec::Current),
        "-" => return Ok(JobSpec::Previous),
        _ => {}
    }
    if rest.starts_with('-') {
        // "%-1", "%-x" — we already matched plain "%-" above, so anything
        // longer starting with '-' is malformed.
        return Err(JobSpecError::BadSymbol);
    }
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return rest
            .parse::<u32>()
            .map(JobSpec::Id)
            .map_err(|_| JobSpecError::BadNumber);
    }
    // v47: substring (%?cmd) or prefix (%cmd).
    if let Some(pattern) = rest.strip_prefix('?') {
        if pattern.is_empty() {
            return Err(JobSpecError::BadSymbol);
        }
        return Ok(JobSpec::Substring(pattern.to_string()));
    }
    Ok(JobSpec::Prefix(rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_percent_alone_is_empty_error() {
        assert_eq!(parse_job_spec("%"), Err(JobSpecError::Empty));
    }

    #[test]
    fn parse_percent_plus_is_current() {
        assert_eq!(parse_job_spec("%+"), Ok(JobSpec::Current));
    }

    #[test]
    fn parse_percent_percent_is_current() {
        assert_eq!(parse_job_spec("%%"), Ok(JobSpec::Current));
    }

    #[test]
    fn parse_percent_minus_is_previous() {
        assert_eq!(parse_job_spec("%-"), Ok(JobSpec::Previous));
    }

    #[test]
    fn parse_percent_digits_is_id() {
        assert_eq!(parse_job_spec("%1"), Ok(JobSpec::Id(1)));
        assert_eq!(parse_job_spec("%42"), Ok(JobSpec::Id(42)));
        assert_eq!(parse_job_spec("%999"), Ok(JobSpec::Id(999)));
    }

    #[test]
    fn parse_percent_digits_with_trailing_garbage_is_bad_number() {
        assert_eq!(parse_job_spec("%1x"), Err(JobSpecError::BadNumber));
        assert_eq!(parse_job_spec("%-1"), Err(JobSpecError::BadSymbol));
    }

    #[test]
    fn parse_percent_letters_is_prefix() {
        assert_eq!(
            parse_job_spec("%abc"),
            Ok(JobSpec::Prefix("abc".to_string()))
        );
    }

    #[test]
    fn parse_percent_tilde_is_prefix() {
        assert_eq!(
            parse_job_spec("%~"),
            Ok(JobSpec::Prefix("~".to_string()))
        );
    }

    #[test]
    fn parse_input_without_percent_is_bad_symbol() {
        // Defensive: callers should not pass non-% input, but if they do,
        // we error rather than panic.
        assert_eq!(parse_job_spec("1"), Err(JobSpecError::BadSymbol));
        assert_eq!(parse_job_spec(""), Err(JobSpecError::BadSymbol));
    }

    #[test]
    fn parse_percent_word_is_prefix() {
        assert_eq!(
            parse_job_spec("%sleep"),
            Ok(JobSpec::Prefix("sleep".to_string()))
        );
    }

    #[test]
    fn parse_percent_question_word_is_substring() {
        assert_eq!(
            parse_job_spec("%?find"),
            Ok(JobSpec::Substring("find".to_string()))
        );
    }

    #[test]
    fn parse_percent_question_alone_is_bad_symbol() {
        assert_eq!(parse_job_spec("%?"), Err(JobSpecError::BadSymbol));
    }

    #[test]
    fn parse_percent_question_with_spaces_in_pattern() {
        assert_eq!(
            parse_job_spec("%?ab cd"),
            Ok(JobSpec::Substring("ab cd".to_string()))
        );
    }
}
