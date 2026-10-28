//! The trigger table: the caches and indexes that Laft computes from the
//! contents of a view while it assembles the view.
//!
//! No package runs code on a client. A cache that depends on what a view
//! contains, such as the font cache or `modules.dep`, is instead produced by
//! a trigger from this fixed table. The builder records in each package's
//! metadata which triggers its payload activates, using
//! [`Trigger::activated_by`], and assembly runs the triggers of a view.

/// The version of [`TRIGGERS`]. It changes whenever an entry changes, so
/// that outputs computed with an older table are never reused.
pub const TRIGGER_TABLE_VERSION: u32 = 1;

/// How many instances of a trigger a view has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instances {
    /// One instance, named after the trigger.
    One,
    /// One instance per kernel release directory `usr/lib/modules/<release>`
    /// that holds a file the trigger's paths match. It is named
    /// `<trigger>:<release>`.
    Kernel,
    /// One instance per icon theme directory `usr/share/icons/<theme>` that
    /// holds an `index.theme`. It is named `<trigger>:<theme>`.
    IconTheme,
}

/// One entry of the trigger table.
///
/// In [`Trigger::args`] and [`Trigger::outputs`], `{}` stands for the
/// instance's release or theme. For a trigger with [`Trigger::each_file`],
/// `{}` in the arguments stands for the absolute path of one file instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    /// The name, which package metadata lists.
    pub name: &'static str,
    /// Globs over payload paths of files and symlinks that activate the
    /// trigger. `*` matches any part of one path component and `**` any
    /// number of components, including none.
    pub paths: &'static [&'static str],
    /// The tool, as absolute paths inside the view. The first one the view
    /// contains runs.
    pub tools: &'static [&'static str],
    /// The arguments of the tool.
    pub args: &'static [&'static str],
    /// A glob of view paths. When set, the tool runs once per matching
    /// file, in sorted order, within one instance.
    pub each_file: Option<&'static str>,
    /// The directories the tool may write, relative to the view tree.
    /// Missing ones are created before it runs.
    pub outputs: &'static [&'static str],
    /// Globs over payload paths of the files the tool generates, which a
    /// package never ships because every view generates its own. A pattern
    /// that starts with `!` exempts the paths it matches. See
    /// [`Trigger::generates`].
    pub generated: &'static [&'static str],
    /// How many instances a view has.
    pub instances: Instances,
    /// Whether a failure fails the assembly. A failed optional trigger is
    /// only reported.
    pub required: bool,
}

impl Trigger {
    /// Whether the payload path `path` of a file or symlink activates this
    /// trigger.
    ///
    /// ```
    /// let depmod = laft_core::trigger("depmod").expect("depmod is a trigger");
    /// assert!(depmod.activated_by("usr/lib/modules/6.17.5-laft1/kernel/fs/ext4/ext4.ko.zst"));
    /// assert!(!depmod.activated_by("usr/lib/modules/6.17.5-laft1/vmlinuz"));
    /// ```
    pub fn activated_by(&self, path: &str) -> bool {
        self.paths.iter().any(|pattern| glob_matches(pattern, path))
    }

    /// Whether the payload path `path` is a file this trigger generates: it
    /// matches a pattern of [`Trigger::generated`] and no exempting `!`
    /// pattern.
    ///
    /// ```
    /// let schemas = laft_core::trigger("glib-schemas").expect("glib-schemas is a trigger");
    /// assert!(schemas.generates("usr/share/glib-2.0/schemas/gschemas.compiled"));
    /// assert!(!schemas.generates("usr/share/glib-2.0/schemas/org.gtk.Settings.xml"));
    /// ```
    pub fn generates(&self, path: &str) -> bool {
        let mut generated = false;
        for pattern in self.generated {
            match pattern.strip_prefix('!') {
                Some(exempt) if glob_matches(exempt, path) => return false,
                Some(_) => {}
                None => generated |= glob_matches(pattern, path),
            }
        }
        generated
    }
}

/// The trigger table, in the order assembly runs the triggers.
pub const TRIGGERS: &[Trigger] = &[
    Trigger {
        name: "depmod",
        paths: &[
            "usr/lib/modules/*/**/*.ko",
            "usr/lib/modules/*/**/*.ko.zst",
            "usr/lib/modules/*/**/*.ko.xz",
        ],
        tools: &["/usr/bin/depmod"],
        args: &["-a", "{}"],
        each_file: None,
        outputs: &["usr/lib/modules/{}"],
        generated: &[
            "usr/lib/modules/*/modules.alias",
            "usr/lib/modules/*/modules.alias.bin",
            "usr/lib/modules/*/modules.builtin.alias.bin",
            "usr/lib/modules/*/modules.builtin.bin",
            "usr/lib/modules/*/modules.dep",
            "usr/lib/modules/*/modules.dep.bin",
            "usr/lib/modules/*/modules.devname",
            "usr/lib/modules/*/modules.softdep",
            "usr/lib/modules/*/modules.symbols",
            "usr/lib/modules/*/modules.symbols.bin",
            "usr/lib/modules/*/modules.weakdep",
        ],
        instances: Instances::Kernel,
        required: true,
    },
    Trigger {
        name: "fontconfig",
        paths: &["usr/share/fonts/**"],
        tools: &["/usr/bin/fc-cache"],
        args: &["-s", "-f"],
        each_file: None,
        outputs: &["usr/lib/fontconfig/cache"],
        generated: &["usr/lib/fontconfig/cache/**"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "glib-schemas",
        paths: &[
            "usr/share/glib-2.0/schemas/*.xml",
            "usr/share/glib-2.0/schemas/*.override",
        ],
        tools: &["/usr/bin/glib-compile-schemas"],
        args: &["/usr/share/glib-2.0/schemas"],
        each_file: None,
        outputs: &["usr/share/glib-2.0/schemas"],
        generated: &["usr/share/glib-2.0/schemas/gschemas.compiled"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "gio-modules",
        paths: &["usr/lib/gio/modules/*.so"],
        tools: &["/usr/bin/gio-querymodules"],
        args: &["/usr/lib/gio/modules"],
        each_file: None,
        outputs: &["usr/lib/gio/modules"],
        generated: &["usr/lib/gio/modules/giomodule.cache"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "gdk-pixbuf",
        paths: &["usr/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.so"],
        tools: &["/usr/bin/gdk-pixbuf-query-loaders"],
        args: &["--update-cache"],
        each_file: None,
        outputs: &["usr/lib/gdk-pixbuf-2.0/2.10.0"],
        generated: &["usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "gtk3-immodules",
        paths: &["usr/lib/gtk-3.0/3.0.0/immodules/*.so"],
        tools: &["/usr/bin/gtk-query-immodules-3.0"],
        args: &["--update-cache"],
        each_file: None,
        outputs: &["usr/lib/gtk-3.0/3.0.0"],
        generated: &["usr/lib/gtk-3.0/3.0.0/immodules.cache"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "mime",
        paths: &["usr/share/mime/packages/*.xml"],
        tools: &["/usr/bin/update-mime-database"],
        args: &["-n", "/usr/share/mime"],
        each_file: None,
        outputs: &["usr/share/mime"],
        generated: &["usr/share/mime/**", "!usr/share/mime/packages/**"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "desktop",
        paths: &["usr/share/applications/*.desktop"],
        tools: &["/usr/bin/update-desktop-database"],
        args: &["-q", "/usr/share/applications"],
        each_file: None,
        outputs: &["usr/share/applications"],
        generated: &["usr/share/applications/mimeinfo.cache"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "icons",
        paths: &["usr/share/icons/*/**"],
        tools: &[
            "/usr/bin/gtk4-update-icon-cache",
            "/usr/bin/gtk-update-icon-cache",
        ],
        args: &["-q", "-t", "-f", "/usr/share/icons/{}"],
        each_file: None,
        outputs: &["usr/share/icons/{}"],
        generated: &["usr/share/icons/*/icon-theme.cache"],
        instances: Instances::IconTheme,
        required: false,
    },
    Trigger {
        name: "udev-hwdb",
        paths: &["usr/lib/udev/hwdb.d/*.hwdb"],
        tools: &["/usr/bin/systemd-hwdb"],
        args: &["update", "--usr"],
        each_file: None,
        outputs: &["usr/lib/udev"],
        generated: &["usr/lib/udev/hwdb.bin"],
        instances: Instances::One,
        required: false,
    },
    Trigger {
        name: "info",
        paths: &["usr/share/info/*.info*"],
        tools: &["/usr/bin/install-info"],
        args: &["{}", "/usr/share/info/dir"],
        each_file: Some("usr/share/info/*.info*"),
        outputs: &["usr/share/info"],
        generated: &["usr/share/info/dir"],
        instances: Instances::One,
        required: false,
    },
];

/// The trigger named `name`, if the table has one.
pub fn trigger(name: &str) -> Option<&'static Trigger> {
    TRIGGERS.iter().find(|t| t.name == name)
}

/// Whether the relative path `path` matches `pattern`, a glob in which `*`
/// matches any part of one path component and `**`, as a whole component,
/// matches any number of components, including none.
///
/// ```
/// assert!(laft_core::glob_matches("usr/share/fonts/**", "usr/share/fonts/ttf/a.ttf"));
/// assert!(laft_core::glob_matches("usr/lib/*/x.so", "usr/lib/gio/x.so"));
/// assert!(!laft_core::glob_matches("usr/lib/*/x.so", "usr/lib/a/b/x.so"));
/// ```
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    components_match(&pattern, &path)
}

fn components_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| components_match(rest, &path[skip..])),
        Some((first, rest)) => path.split_first().is_some_and(|(part, tail)| {
            component_matches(first, part) && components_match(rest, tail)
        }),
    }
}

/// Matches one component against a pattern whose only wildcard is `*`.
fn component_matches(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&b| b == b'*')
}

#[cfg(test)]
#[path = "trigger_tests.rs"]
mod tests;
