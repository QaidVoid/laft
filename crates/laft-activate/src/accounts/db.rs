//! Parsing and printing of `/etc/passwd`, `/etc/group`, `/etc/shadow` and
//! `/etc/gshadow`.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use crate::error::{Error, Result, Warning, io};

/// A line of `/etc/passwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Passwd {
    pub(crate) name: String,
    pub(crate) password: String,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) gecos: String,
    pub(crate) home: String,
    pub(crate) shell: String,
}

/// A line of `/etc/group`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupLine {
    pub(crate) name: String,
    pub(crate) password: String,
    pub(crate) gid: u32,
    pub(crate) members: Vec<String>,
}

/// A line of `/etc/shadow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shadow {
    pub(crate) name: String,
    pub(crate) hash: String,
    /// The seven aging fields after the hash, verbatim.
    pub(crate) aging: Vec<String>,
}

/// A line of `/etc/gshadow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Gshadow {
    pub(crate) name: String,
    pub(crate) password: String,
    pub(crate) admins: String,
    pub(crate) members: Vec<String>,
}

/// The four account databases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Databases {
    pub(crate) passwd: Vec<Passwd>,
    pub(crate) group: Vec<GroupLine>,
    pub(crate) shadow: Vec<Shadow>,
    pub(crate) gshadow: Vec<Gshadow>,
    /// Lines that were read with missing trailing fields.
    pub(crate) warnings: Vec<Warning>,
}

impl Databases {
    /// Reads the databases in the directory `etc`. A missing database is
    /// empty. A line with fewer fields than its database has, as other
    /// tools may write, reads the missing trailing fields as empty and adds
    /// a warning. A missing password field of `shadow` or `gshadow` reads
    /// as `!`, a locked password, because an empty one needs no password.
    pub(crate) fn read(etc: &Path) -> Result<Self> {
        let mut warnings = Vec::new();
        let w = &mut warnings;
        Ok(Self {
            passwd: parse(&etc.join("passwd"), 7, "", w, |f| {
                Ok(Passwd {
                    name: f[0].to_owned(),
                    password: f[1].to_owned(),
                    uid: id(f[2])?,
                    gid: id(f[3])?,
                    gecos: f[4].to_owned(),
                    home: f[5].to_owned(),
                    shell: f[6].to_owned(),
                })
            })?,
            group: parse(&etc.join("group"), 4, "", w, |f| {
                Ok(GroupLine {
                    name: f[0].to_owned(),
                    password: f[1].to_owned(),
                    gid: id(f[2])?,
                    members: list(f[3]),
                })
            })?,
            shadow: parse(&etc.join("shadow"), 9, LOCKED, w, |f| {
                Ok(Shadow {
                    name: f[0].to_owned(),
                    hash: f[1].to_owned(),
                    aging: f[2..].iter().map(|s| (*s).to_owned()).collect(),
                })
            })?,
            gshadow: parse(&etc.join("gshadow"), 4, LOCKED, w, |f| {
                Ok(Gshadow {
                    name: f[0].to_owned(),
                    password: f[1].to_owned(),
                    admins: f[2].to_owned(),
                    members: list(f[3]),
                })
            })?,
            warnings,
        })
    }

    /// The text of each database, by file name.
    pub(crate) fn render(&self) -> [(&'static str, String); 4] {
        let mut passwd = String::new();
        for p in &self.passwd {
            let fields = [
                &p.name,
                &p.password,
                &p.uid.to_string(),
                &p.gid.to_string(),
                &p.gecos,
                &p.home,
                &p.shell,
            ];
            line(&mut passwd, &fields);
        }
        let mut group = String::new();
        for g in &self.group {
            let fields = [
                &g.name,
                &g.password,
                &g.gid.to_string(),
                &g.members.join(","),
            ];
            line(&mut group, &fields);
        }
        let mut shadow = String::new();
        for s in &self.shadow {
            let mut fields = vec![&s.name, &s.hash];
            fields.extend(&s.aging);
            line(&mut shadow, &fields);
        }
        let mut gshadow = String::new();
        for g in &self.gshadow {
            let fields = [&g.name, &g.password, &g.admins, &g.members.join(",")];
            line(&mut gshadow, &fields);
        }
        [
            ("passwd", passwd),
            ("group", group),
            ("shadow", shadow),
            ("gshadow", gshadow),
        ]
    }
}

fn line(out: &mut String, fields: &[&String]) {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(field);
    }
    out.push('\n');
}

/// A password field that matches no password.
const LOCKED: &str = "!";

/// Parses every non-empty line of the database `path` into `fields` fields.
/// A missing password field, the second one, reads as `password`. Other
/// missing trailing fields are empty. Each line with missing fields adds a
/// warning to `warnings`. A line without a name, with too many fields, a
/// bad value or a name seen before is malformed.
fn parse<T>(
    path: &Path,
    fields: usize,
    password: &'static str,
    warnings: &mut Vec<Warning>,
    build: impl Fn(&[&str]) -> Result<T, &'static str>,
) -> Result<Vec<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(path)(e)),
    };
    let mut names = BTreeSet::new();
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let malformed = |reason: String| Error::Malformed {
            path: path.to_path_buf(),
            line: index + 1,
            reason,
        };
        let mut parts: Vec<&str> = line.split(':').collect();
        if parts[0].is_empty() {
            return Err(malformed("the line names no account".to_owned()));
        }
        if parts.len() > fields {
            return Err(malformed(format!(
                "expected {fields} fields, found {}",
                parts.len()
            )));
        }
        if parts.len() < fields {
            if parts.len() == 1 {
                parts.push(password);
            }
            parts.resize(fields, "");
            warnings.push(Warning::ShortLine {
                path: path.to_path_buf(),
                line: index + 1,
            });
        }
        if !names.insert(parts[0]) {
            return Err(malformed(format!("{:?} is listed twice", parts[0])));
        }
        entries.push(build(&parts).map_err(|reason| malformed(reason.to_owned()))?);
    }
    Ok(entries)
}

/// A decimal id: ASCII digits without a leading zero, or `0` itself, so
/// that every id has one spelling and a sign is malformed rather than
/// dropped when the database is written back.
fn id(text: &str) -> Result<u32, &'static str> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) {
        return Err("invalid id");
    }
    text.parse().map_err(|_| "invalid id")
}

fn list(text: &str) -> Vec<String> {
    text.split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
