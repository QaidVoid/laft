/// Whether a registry account is a user, a group or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Group,
    UserAndGroup,
}

/// The static system accounts: name, id and kind.
///
/// These ids are the same on every Laft system, so package files can be owned
/// by them. Ids are never reassigned or reused, and new accounts are only
/// appended.
const REGISTRY: &[(&str, u32, Kind)] = &[
    ("root", 0, Kind::UserAndGroup),
    ("tty", 5, Kind::Group),
    ("disk", 6, Kind::Group),
    ("lp", 7, Kind::Group),
    ("kmem", 9, Kind::Group),
    ("wheel", 10, Kind::Group),
    ("cdrom", 11, Kind::Group),
    ("mail", 12, Kind::Group),
    ("dialout", 18, Kind::Group),
    ("floppy", 19, Kind::Group),
    ("audio", 20, Kind::Group),
    ("video", 21, Kind::Group),
    ("utmp", 22, Kind::Group),
    ("input", 23, Kind::Group),
    ("kvm", 24, Kind::Group),
    ("render", 25, Kind::Group),
    ("tape", 26, Kind::Group),
    ("sgx", 27, Kind::Group),
    ("messagebus", 30, Kind::UserAndGroup),
    ("sshd", 31, Kind::UserAndGroup),
    ("dhcpcd", 32, Kind::UserAndGroup),
    ("chrony", 33, Kind::UserAndGroup),
    ("polkitd", 34, Kind::UserAndGroup),
    ("rtkit", 35, Kind::UserAndGroup),
    ("uuidd", 36, Kind::UserAndGroup),
    ("dnsmasq", 37, Kind::UserAndGroup),
    ("avahi", 38, Kind::UserAndGroup),
    ("tss", 39, Kind::UserAndGroup),
    ("log", 40, Kind::UserAndGroup),
    ("shadow", 41, Kind::Group),
    ("plugdev", 42, Kind::Group),
    ("users", 100, Kind::Group),
    ("nobody", 65534, Kind::User),
    ("nogroup", 65534, Kind::Group),
];

/// The fixed uid of the system user `name`, or `None` when `name` is not a
/// user in the static system account registry.
///
/// Only registry accounts may own package files, because their ids are the
/// same on every system.
///
/// ```
/// assert_eq!(laft_core::system_uid("sshd"), Some(31));
/// assert_eq!(laft_core::system_uid("wheel"), None);
/// ```
pub fn system_uid(name: &str) -> Option<u32> {
    lookup(name, Kind::User)
}

/// The fixed gid of the system group `name`, or `None` when `name` is not a
/// group in the static system account registry.
///
/// ```
/// assert_eq!(laft_core::system_gid("wheel"), Some(10));
/// assert_eq!(laft_core::system_gid("nobody"), None);
/// ```
pub fn system_gid(name: &str) -> Option<u32> {
    lookup(name, Kind::Group)
}

fn lookup(name: &str, wanted: Kind) -> Option<u32> {
    REGISTRY
        .iter()
        .find(|&&(n, _, kind)| n == name && (kind == wanted || kind == Kind::UserAndGroup))
        .map(|&(_, id, _)| id)
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;
