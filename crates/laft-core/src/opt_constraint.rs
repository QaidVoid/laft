//! The option constraints of a dependency: `flag`, `!flag` and `opt=tag`
//! in braces after a package name.

use std::fmt;
use std::str::FromStr;

use crate::{OptionValue, ParseError, is_valid_name};

/// One requirement a dependency puts on an option of the package it names.
///
/// | Text | Requires |
/// |---|---|
/// | `llvm` | the option `llvm` is `true` |
/// | `!x11` | the option `x11` is `false` |
/// | `vulkan=amd` | the option `vulkan` has the tag `amd` |
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OptionConstraint {
    /// `flag`: the option must be `true`.
    True {
        /// The option name.
        name: String,
    },
    /// `!flag`: the option must be `false`.
    False {
        /// The option name.
        name: String,
    },
    /// `opt=tag`: the option must have this tag.
    Tag {
        /// The option name.
        name: String,
        /// The required tag.
        tag: String,
    },
}

impl OptionConstraint {
    /// The option the constraint is about.
    pub fn name(&self) -> &str {
        match self {
            Self::True { name } | Self::False { name } | Self::Tag { name, .. } => name,
        }
    }

    /// The tag the option must have, for a constraint written `opt=tag`.
    pub fn tag(&self) -> Option<&str> {
        match self {
            Self::True { .. } | Self::False { .. } => None,
            Self::Tag { tag, .. } => Some(tag),
        }
    }

    /// Whether `value`, the value of the option, satisfies the constraint.
    pub fn matches(&self, value: &OptionValue) -> bool {
        match (self, value) {
            (Self::True { .. }, OptionValue::Bool(true)) => true,
            (Self::False { .. }, OptionValue::Bool(false)) => true,
            (Self::Tag { tag, .. }, OptionValue::Tag(own)) => own == tag,
            _ => false,
        }
    }

    /// The constraint for `name=tag`, which spells booleans without `=`.
    fn tag_constraint(name: &str, tag: &str) -> Option<Self> {
        (is_valid_name(name) && is_valid_name(tag) && !matches!(tag, "true" | "false")).then(|| {
            Self::Tag {
                name: name.to_owned(),
                tag: tag.to_owned(),
            }
        })
    }

    /// The constraint for `flag` or `!flag`. The names `true` and `false`
    /// are the bool literals, so no option can be named them.
    fn flag_constraint(name: &str, value: bool) -> Option<Self> {
        (is_valid_name(name) && !matches!(name, "true" | "false")).then(|| {
            let name = name.to_owned();
            if value {
                Self::True { name }
            } else {
                Self::False { name }
            }
        })
    }
}

impl FromStr for OptionConstraint {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let constraint = match s.split_once('=') {
            Some((name, tag)) => Self::tag_constraint(name, tag),
            None if s.starts_with('!') => Self::flag_constraint(&s[1..], false),
            None => Self::flag_constraint(s, true),
        };
        constraint.ok_or_else(|| {
            ParseError::new(
                "option constraint",
                s,
                "expected `flag`, `!flag` or `opt=tag` of lowercase names, where the option name and the tag are not `true` or `false`",
            )
        })
    }
}

impl fmt::Display for OptionConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::True { name } => f.write_str(name),
            Self::False { name } => write!(f, "!{name}"),
            Self::Tag { name, tag } => write!(f, "{name}={tag}"),
        }
    }
}

/// The option constraints of one dependency, in the order they are
/// written, written `{flag,!flag,opt=tag}`. An empty list constrains
/// nothing, and no option is named twice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct OptionConstraints(Vec<OptionConstraint>);

impl OptionConstraints {
    /// The constraints as a slice.
    pub fn as_slice(&self) -> &[OptionConstraint] {
        &self.0
    }

    /// Whether the list constrains nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<OptionConstraint>> for OptionConstraints {
    fn from(constraints: Vec<OptionConstraint>) -> Self {
        Self(constraints)
    }
}

impl FromStr for OptionConstraints {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let inner = s
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .filter(|inner| !inner.is_empty())
            .ok_or_else(|| {
                ParseError::new(
                    "option constraints",
                    s,
                    "expected `{flag,...}` with at least one constraint and no spaces",
                )
            })?;
        let constraints = inner
            .split(',')
            .map(OptionConstraint::from_str)
            .collect::<Result<Vec<_>, _>>()?;
        let mut names: Vec<&str> = Vec::with_capacity(constraints.len());
        for constraint in &constraints {
            let name = constraint.name();
            if names.contains(&name) {
                return Err(ParseError::new(
                    "option constraints",
                    s,
                    "each option may be constrained at most once",
                ));
            }
            names.push(name);
        }
        Ok(Self(constraints))
    }
}

impl fmt::Display for OptionConstraints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return Ok(());
        }
        f.write_str("{")?;
        for (i, constraint) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{constraint}")?;
        }
        f.write_str("}")
    }
}

/// Splits a trailing balanced `{...}` group off `s`, and returns the head
/// and the group with its braces. Without a closing brace the whole text
/// counts as the head, so the head fails to parse instead.
pub(crate) fn split_tail_group(s: &str) -> (&str, Option<&str>) {
    if !s.ends_with('}') {
        return (s, None);
    }
    let mut depth = 0usize;
    for (i, c) in s.char_indices().rev() {
        match c {
            '}' => depth += 1,
            '{' => {
                depth -= 1;
                if depth == 0 {
                    return (&s[..i], Some(&s[i..]));
                }
            }
            _ => {}
        }
    }
    (s, None)
}

#[cfg(test)]
#[path = "opt_constraint_tests.rs"]
mod tests;
