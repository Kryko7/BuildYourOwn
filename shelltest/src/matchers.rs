//! Text matchers used by every `expect` field.

use anyhow::{bail, Result};
use serde::de::{Deserialize, Deserializer, Error as DeError};
use serde_yaml::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Matcher {
    Exact(String),
    Regex(String),
    Contains(String),
    NotContains(String),
    LinesUnordered(Vec<String>),
    LinesOrderedSubset(Vec<String>),
}

const KEYS: &str =
    "exact | regex | contains | not_contains | lines_unordered | lines_ordered_subset";

impl<'de> Deserialize<'de> for Matcher {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Matcher::from_value(&v).map_err(D::Error::custom)
    }
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

fn lines(v: &Value, key: &str) -> Result<Vec<String>> {
    match v {
        Value::Sequence(items) => items
            .iter()
            .map(|i| scalar(i).ok_or_else(|| anyhow::anyhow!("'{key}' entries must be strings")))
            .collect(),
        _ => bail!("'{key}' expects a list of lines"),
    }
}

impl Matcher {
    pub fn from_value(v: &Value) -> Result<Matcher> {
        if let Some(s) = scalar(v) {
            return Ok(Matcher::Exact(s));
        }
        let Value::Mapping(map) = v else {
            bail!("matcher must be a string or a single-key map ({KEYS})");
        };
        if map.len() != 1 {
            bail!("matcher map must have exactly one key ({KEYS})");
        }
        let (k, val) = map.iter().next().unwrap();
        let key = k.as_str().unwrap_or_default();
        let text = || scalar(val).ok_or_else(|| anyhow::anyhow!("'{key}' expects a string"));
        Ok(match key {
            "exact" => Matcher::Exact(text()?),
            "regex" => {
                let r = text()?;
                regex::Regex::new(&r).map_err(|e| anyhow::anyhow!("invalid regex '{r}': {e}"))?;
                Matcher::Regex(r)
            }
            "contains" => Matcher::Contains(text()?),
            "not_contains" => Matcher::NotContains(text()?),
            "lines_unordered" => Matcher::LinesUnordered(lines(val, key)?),
            "lines_ordered_subset" => Matcher::LinesOrderedSubset(lines(val, key)?),
            other => bail!("unknown matcher '{other}' (expected one of: {KEYS})"),
        })
    }

    pub fn map_text(&self, f: impl Fn(&str) -> String) -> Matcher {
        let vec = |v: &Vec<String>| v.iter().map(|s| f(s)).collect();
        match self {
            Matcher::Exact(s) => Matcher::Exact(f(s)),
            Matcher::Regex(s) => Matcher::Regex(f(s)),
            Matcher::Contains(s) => Matcher::Contains(f(s)),
            Matcher::NotContains(s) => Matcher::NotContains(f(s)),
            Matcher::LinesUnordered(v) => Matcher::LinesUnordered(vec(v)),
            Matcher::LinesOrderedSubset(v) => Matcher::LinesOrderedSubset(vec(v)),
        }
    }

    /// Apply `f` only to text compared line-for-line (exact and line-list matchers).
    pub fn map_exact(&self, f: impl Fn(&str) -> String) -> Matcher {
        match self {
            Matcher::Exact(_) | Matcher::LinesUnordered(_) | Matcher::LinesOrderedSubset(_) => {
                self.map_text(f)
            }
            other => other.clone(),
        }
    }

    /// Human-readable description of the expectation for reports.
    pub fn describe(&self) -> String {
        match self {
            Matcher::Exact(s) => format!("{s:?}"),
            Matcher::Regex(s) => format!("matches regex {s:?}"),
            Matcher::Contains(s) => format!("contains {s:?}"),
            Matcher::NotContains(s) => format!("does not contain {s:?}"),
            Matcher::LinesUnordered(v) => format!("lines (any order) {v:?}"),
            Matcher::LinesOrderedSubset(v) => format!("lines in order (subset) {v:?}"),
        }
    }

    /// The text to diff against when the matcher is line-based.
    pub fn expected_text(&self) -> Option<String> {
        match self {
            Matcher::Exact(s) => Some(s.clone()),
            Matcher::LinesUnordered(v) | Matcher::LinesOrderedSubset(v) => Some(v.join("\n")),
            _ => None,
        }
    }

    pub fn check(&self, actual: &str) -> std::result::Result<(), String> {
        match self {
            Matcher::Exact(e) => (actual == e)
                .then_some(())
                .ok_or_else(|| "output differs".into()),
            Matcher::Regex(r) => {
                let re = regex::Regex::new(r).map_err(|e| e.to_string())?;
                re.is_match(actual)
                    .then_some(())
                    .ok_or_else(|| format!("regex {r:?} did not match"))
            }
            Matcher::Contains(s) => actual
                .contains(s.as_str())
                .then_some(())
                .ok_or_else(|| format!("expected substring {s:?} not found")),
            Matcher::NotContains(s) => (!actual.contains(s.as_str()))
                .then_some(())
                .ok_or_else(|| format!("forbidden substring {s:?} was found")),
            Matcher::LinesUnordered(exp) => {
                let mut a: Vec<&str> = actual.lines().collect();
                let mut e: Vec<&str> = exp.iter().map(String::as_str).collect();
                a.sort_unstable();
                e.sort_unstable();
                (a == e)
                    .then_some(())
                    .ok_or_else(|| "line multiset differs".into())
            }
            Matcher::LinesOrderedSubset(exp) => {
                let mut it = actual.lines();
                for want in exp {
                    if !it.any(|l| l == want) {
                        return Err(format!("line {want:?} missing (or out of order)"));
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(yaml: &str) -> Matcher {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn parses_forms() {
        assert_eq!(m("hello"), Matcher::Exact("hello".into()));
        assert_eq!(m("0"), Matcher::Exact("0".into()));
        assert_eq!(m("{contains: hi}"), Matcher::Contains("hi".into()));
        assert_eq!(
            m("{lines_unordered: [a, b]}"),
            Matcher::LinesUnordered(vec!["a".into(), "b".into()])
        );
        assert!(serde_yaml::from_str::<Matcher>("{bogus: x}").is_err());
        assert!(serde_yaml::from_str::<Matcher>("{contains: a, regex: b}").is_err());
        assert!(serde_yaml::from_str::<Matcher>("{regex: '('}").is_err());
        assert_eq!(
            m("{contains: 'a '}").map_exact(|s| s.trim().into()),
            Matcher::Contains("a ".into())
        );
        assert_eq!(
            m("'a '").map_exact(|s| s.trim().into()),
            Matcher::Exact("a".into())
        );
    }

    #[test]
    fn checks() {
        assert!(m("{regex: '^a.c$'}").check("abc").is_ok());
        assert!(m("{not_contains: x}").check("abc").is_ok());
        assert!(m("{not_contains: b}").check("abc").is_err());
        assert!(m("{lines_unordered: [b, a]}").check("a\nb").is_ok());
        assert!(m("{lines_unordered: [b, a]}").check("a\nb\nc").is_err());
        assert!(m("{lines_ordered_subset: [a, c]}").check("a\nb\nc").is_ok());
        assert!(m("{lines_ordered_subset: [c, a]}")
            .check("a\nb\nc")
            .is_err());
    }
}
