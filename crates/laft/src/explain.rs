//! The long explanations of Laft's diagnostic codes, for `laft explain`.

/// The explanation of every code: what it means and how to fix it.
const TEXTS: &[(&str, &str, &str)] = &[
    (
        "C0405",
        "A user or group name is not valid.",
        "Use a name of lowercase letters, digits, `_` and `-` that starts with a letter, at most 32 characters.",
    ),
    (
        "C0406",
        "A user sets both `password_hash` and `password_file`.",
        "Keep one of them.",
    ),
    (
        "C0407",
        "An explicit uid or gid lies in a range reserved for the static system accounts.",
        "Leave the id out, so that Laft allocates one, or pick one from 1000 on.",
    ),
    (
        "C0408",
        "The id of `root` or of a static account was changed.",
        "Remove the id. Static accounts have the same ids on every Laft system.",
    ),
    (
        "C0414",
        "Two declared accounts have the same id.",
        "Give one of them another id, or leave both out.",
    ),
    (
        "C0415",
        "A regular user would take a system group as its primary group.",
        "Set the user's `group` explicitly.",
    ),
    (
        "C0420",
        "A `file` path is not an absolute, normalized path below `/etc/`.",
        "Write it as `/etc/<path>` without `.`, `..` or repeated slashes.",
    ),
    (
        "C0421",
        "A `file` path is reserved for Laft, such as `/etc/static` or an account database.",
        "Configure the setting the file belongs to instead.",
    ),
    (
        "C0422",
        "An enabled `file` does not set exactly one of `text`, `source`, `link`, `json`, `toml`, `yaml`, `ini` and `env`.",
        "Keep exactly one content field.",
    ),
    (
        "C0425",
        "A path is both a file and the directory of another entry.",
        "Move one of the files.",
    ),
    (
        "C0426",
        "Structured content cannot be written in the format of its file.",
        "Use values the format can express, for example no null in TOML.",
    ),
    (
        "C0427",
        "An `env` key is not a variable name.",
        "Use letters, digits and `_`, not starting with a digit.",
    ),
    (
        "C0431",
        "A sysctl key is malformed.",
        "Write it with dots, such as `net.ipv4.ip_forward`.",
    ),
    (
        "C0433",
        "A kernel module is both loaded and blacklisted.",
        "Remove it from one of `kernel.modules` and `kernel.blacklist`.",
    ),
    (
        "C0434",
        "A sysctl value is not an integer, string or boolean.",
        "Write the value the kernel expects.",
    ),
    (
        "C0450",
        "The init backend is not available in this version.",
        "Remove the line or write `init = runit`.",
    ),
    (
        "C0490",
        "A value cannot be written into the file it belongs to.",
        "Remove characters such as newlines or `:` that the file format cannot hold.",
    ),
    (
        "C0491",
        "No free id is left for an account.",
        "Set an explicit id.",
    ),
    (
        "C0492",
        "A reference names an account that is not declared.",
        "Declare the account or fix the name.",
    ),
    (
        "C0801",
        "A repository URL has an unsupported scheme, or uses `http://` without `allow_http`.",
        "Use `https://` or `file://`, or set `allow_http = true` for a trusted network.",
    ),
    (
        "C0802",
        "A repository key is not a minisign public key.",
        "Paste the base64 line of the repository's `.pub` file.",
    ),
    (
        "C0803",
        "A repository name is not a package name.",
        "Use lowercase letters, digits and `-`.",
    ),
    (
        "C0901",
        "A package reference does not resolve against the synced indexes.",
        "Fix the name, or run `laft repo sync` when the package is new.",
    ),
    (
        "C0902",
        "An alternatives group is not a name, or a member is not a bare package name.",
        "Write `alternatives.vi = neovim` without version or variant.",
    ),
    (
        "C0903",
        "A view name is not a package name, or is `system`.",
        "Rename the view.",
    ),
    (
        "C0904",
        "A view allows a user or group that is not declared.",
        "Declare the account or remove it from the view.",
    ),
    (
        "C1101",
        "No root filesystem is declared.",
        "Add `fs \"/\" { device = ..., fstype = ... }`, usually in hardware.stav.",
    ),
    (
        "C1102",
        "A mountpoint is not an absolute, normalized path.",
        "Write it as `/path` without `.` or `..`.",
    ),
    (
        "C1103",
        "A mountpoint is managed by Laft, such as `/usr` or `/run`.",
        "Remove the filesystem.",
    ),
    (
        "C1104",
        "The root filesystem type cannot be mounted by the initramfs.",
        "Use ext4, xfs, btrfs or f2fs.",
    ),
    (
        "C1105",
        "A device of the root, `/laft`, the ESP or a LUKS block cannot be resolved by the initramfs, or a LUKS name is invalid.",
        "Use `UUID=`, `PARTUUID=`, `LABEL=` or a `/dev/mapper/` name.",
    ),
    (
        "C1106",
        "The root filesystem is mounted read-only.",
        "Remove `ro` from its options.",
    ),
    (
        "C1107",
        "`boot.esp` has no `vfat` filesystem.",
        "Declare the ESP as `fs \"/boot\" { fstype = vfat, ... }`.",
    ),
    (
        "C1108",
        "A `/dev/mapper/<name>` device has no `luks <name>` block.",
        "Declare the LUKS device.",
    ),
    ("C1109", "A filesystem has no device.", "Set `device`."),
    (
        "C1110",
        "A swap device is not a path or a `UUID=`, `LABEL=`, `PARTUUID=` or `PARTLABEL=` tag.",
        "Fix the device.",
    ),
    (
        "C1201",
        "A service name is invalid or reserved.",
        "Use another name.",
    ),
    (
        "C1202",
        "A getty terminal is invalid or repeated.",
        "List each terminal once, such as `tty1`.",
    ),
    (
        "C1203",
        "A service setting does not fit its kind.",
        "Use `exec` for longruns and `up` and `down` for oneshots.",
    ),
    (
        "C1204",
        "A command or path is not absolute.",
        "Write the full path, such as `/usr/bin/sshd`.",
    ),
    (
        "C1205",
        "A service depends on a disabled service, or an early service on a later one.",
        "Enable the dependency or remove it from `after`.",
    ),
    (
        "C1206",
        "Services depend on each other in a cycle.",
        "Remove one of the `after` entries.",
    ),
    (
        "C1207",
        "An environment variable cannot be passed to a program.",
        "Use a variable name without `=` and a value without NUL.",
    ),
    (
        "C1208",
        "Services share a directory with different owners or modes.",
        "Give them the same owner and mode, or separate directories.",
    ),
    (
        "C1209",
        "A service package is not a bare package name.",
        "Write the name without version or variant.",
    ),
    (
        "C1210",
        "`early` is set on a service outside the base part.",
        "Remove `early`. Only the base part defines the early boot graph.",
    ),
    (
        "C1301",
        "A privileged file lies outside `/usr/bin` and `/usr/lib`.",
        "Grant privileges only to programs.",
    ),
    (
        "C1302",
        "A privileged file grants nothing.",
        "Set a setuid or setgid mode or capabilities, or remove the entry.",
    ),
    (
        "C1303",
        "The capability text is invalid.",
        "Write it like `cap_net_raw=ep`.",
    ),
    (
        "R001",
        "No candidate matches a request.",
        "Check the name, version prefix and variant with `laft search`, or sync the indexes.",
    ),
    (
        "R002",
        "Requests for the same package disagree on the variant or the version prefix.",
        "Request the package once per view.",
    ),
    (
        "R003",
        "Nothing satisfies a dependency of a selected package.",
        "The repository is incomplete. Sync again later, or pin the package to an older version.",
    ),
    (
        "R004",
        "Several packages provide a dependency and nothing says which one to use.",
        "Request the provider you want in the view.",
    ),
    (
        "R005",
        "The constraints on a package leave no candidate.",
        "Relax one of the listed constraints, for example a version prefix.",
    ),
    (
        "R006",
        "Two selected packages conflict, or one replaces the other.",
        "Remove one of the requests that pull them in.",
    ),
    (
        "R007",
        "The only providers of a dependency are selected in a version or variant that does not provide it.",
        "Request the provider in the version that provides it.",
    ),
    (
        "R008",
        "Several repositories of the same, highest priority list a package.",
        "Give the repositories different priorities.",
    ),
    (
        "R009",
        "The resolver gave up after too many passes.",
        "Request more packages explicitly to narrow the search.",
    ),
    (
        "R010",
        "An installed package was yanked and nothing else satisfies what needs it.",
        "Wait for a fixed package or pin another version.",
    ),
    (
        "W001",
        "A requested package was renamed, and the request now selects the package that replaces it.",
        "Update the configuration to the new name.",
    ),
    (
        "W002",
        "An installed package is in no repository any more and is kept.",
        "Remove the request or find a replacement.",
    ),
    (
        "W003",
        "An installed version was yanked, so the resolution moves to another one.",
        "Nothing to do. The reason is shown with the warning.",
    ),
    (
        "KR001",
        "Two packages that can share a view own the same path outside a common alternatives group.",
        "Declare an alternatives group or move a file.",
    ),
    (
        "KR002",
        "A dependency is satisfied by no package of the index or a base index.",
        "Add the missing package.",
    ),
    (
        "KR003",
        "A package cannot be installed on its own.",
        "Fix its dependencies.",
    ),
    (
        "KR004",
        "An alternatives group has no valid default, or its members ship different paths.",
        "Set a default with `--alternative` and make the members agree.",
    ),
    (
        "KR005",
        "A package version is published again with another store id.",
        "Bump the revision.",
    ),
    (
        "KR006",
        "The newest version of a package is older than one published before.",
        "Check that the downgrade is intended.",
    ),
    (
        "KR007",
        "Several packages provide the same `so:` or `virtual:` value without a provider default.",
        "Set one with `--provider`.",
    ),
    (
        "KR008",
        "A package file has invalid metadata or manifest.",
        "Rebuild the package.",
    ),
    (
        "KR009",
        "A package file is missing or differs from its entry.",
        "Publish the file again.",
    ),
    (
        "KR010",
        "An entry's metadata or store id differs from its package file.",
        "Index the package file again.",
    ),
    (
        "KR011",
        "A payload ships a file that a trigger generates.",
        "Remove the generated file from the payload.",
    ),
    (
        "CI001",
        "A package was built from a dirty recipe tree or by a bootstrap build.",
        "Rebuild from a clean tree, or allow it explicitly.",
    ),
    (
        "CI003",
        "A package has no readable buildinfo.",
        "Index the build output directory, which holds the buildinfo.",
    ),
];

/// The explanation of `code`, or `None` for an unknown code.
pub(crate) fn explain(code: &str) -> Option<String> {
    let code = code.to_ascii_uppercase();
    TEXTS
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(c, what, fix)| format!("{c}: {what}\n\nfix: {fix}\n"))
}

#[cfg(test)]
#[path = "explain_tests.rs"]
mod tests;
