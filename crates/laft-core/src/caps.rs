use std::str::FromStr;

use crate::ParseError;

/// The capability names, indexed by capability number.
const NAMES: [&str; 41] = [
    "cap_chown",
    "cap_dac_override",
    "cap_dac_read_search",
    "cap_fowner",
    "cap_fsetid",
    "cap_kill",
    "cap_setgid",
    "cap_setuid",
    "cap_setpcap",
    "cap_linux_immutable",
    "cap_net_bind_service",
    "cap_net_broadcast",
    "cap_net_admin",
    "cap_net_raw",
    "cap_ipc_lock",
    "cap_ipc_owner",
    "cap_sys_module",
    "cap_sys_rawio",
    "cap_sys_chroot",
    "cap_sys_ptrace",
    "cap_sys_pacct",
    "cap_sys_admin",
    "cap_sys_boot",
    "cap_sys_nice",
    "cap_sys_resource",
    "cap_sys_time",
    "cap_sys_tty_config",
    "cap_mknod",
    "cap_lease",
    "cap_audit_write",
    "cap_audit_control",
    "cap_setfcap",
    "cap_mac_override",
    "cap_mac_admin",
    "cap_syslog",
    "cap_wake_alarm",
    "cap_block_suspend",
    "cap_audit_read",
    "cap_perfmon",
    "cap_bpf",
    "cap_checkpoint_restore",
];

/// `VFS_CAP_REVISION_2`, the format of `security.capability` with 64
/// capability bits.
const REVISION_2: u32 = 0x0200_0000;

/// `VFS_CAP_FLAGS_EFFECTIVE`.
const FLAG_EFFECTIVE: u32 = 0x1;

/// The file capabilities of a privileged program, parsed from the text
/// syntax of `cap_from_text(3)`, for example `cap_setuid=ep` or
/// `cap_net_raw,cap_net_admin+ep`.
///
/// The text is a list of clauses separated by spaces. Each clause is a
/// comma separated list of capability names, or `all`, followed by one or
/// more operations: `=` replaces the flags of the listed capabilities, `+`
/// adds flags and `-` removes them. The flags are `e`, `i` and `p`. An empty
/// list before `=` means every capability.
///
/// A file has a single effective flag, so the effective set must be empty
/// or equal to the union of the permitted and inheritable sets.
///
/// ```
/// use laft_core::FileCapabilities;
///
/// let caps: FileCapabilities = "cap_setuid=ep".parse()?;
/// assert_eq!(caps.permitted, 1 << 7);
/// assert!(caps.effective);
/// assert_eq!(caps.to_xattr().len(), 20);
/// # Ok::<(), laft_core::ParseError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileCapabilities {
    /// The permitted set, one bit per capability number.
    pub permitted: u64,
    /// The inheritable set.
    pub inheritable: u64,
    /// Whether the permitted capabilities become effective at `execve`.
    pub effective: bool,
}

impl FileCapabilities {
    /// The value of the `security.capability` extended attribute in the
    /// `VFS_CAP_REVISION_2` format: a little endian magic word with the
    /// effective flag, then the low and high 32 bits of the permitted and
    /// inheritable sets.
    pub fn to_xattr(&self) -> [u8; 20] {
        let mut magic = REVISION_2;
        if self.effective {
            magic |= FLAG_EFFECTIVE;
        }
        let words = [
            magic,
            self.permitted as u32,
            self.inheritable as u32,
            (self.permitted >> 32) as u32,
            (self.inheritable >> 32) as u32,
        ];
        let mut out = [0u8; 20];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }
}

impl FromStr for FileCapabilities {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let error = |reason| ParseError::new("capability text", s, reason);
        let all = (1u64 << NAMES.len()) - 1;
        let (mut e, mut i, mut p) = (0u64, 0u64, 0u64);
        let mut clauses = s.split_ascii_whitespace().peekable();
        if clauses.peek().is_none() {
            return Err(error("expected at least one clause"));
        }
        for clause in clauses {
            let Some(split) = clause.find(['=', '+', '-']) else {
                return Err(error("a clause needs an operation `=`, `+` or `-`"));
            };
            let (list, mut actions) = clause.split_at(split);
            let mut set = 0u64;
            if list.is_empty() {
                if !actions.starts_with('=') {
                    return Err(error("only `=` may follow an empty capability list"));
                }
                set = all;
            }
            for name in list.split(',').filter(|_| !list.is_empty()) {
                let name = name.to_ascii_lowercase();
                set |= if name == "all" {
                    all
                } else {
                    let number = NAMES
                        .iter()
                        .position(|&n| n == name)
                        .ok_or_else(|| error("unknown capability name"))?;
                    1 << number
                };
            }
            while let Some(op) = actions.chars().next() {
                let rest = &actions[1..];
                let end = rest.find(['=', '+', '-']).unwrap_or(rest.len());
                let flags = &rest[..end];
                actions = &rest[end..];
                if op == '=' {
                    e &= !set;
                    i &= !set;
                    p &= !set;
                } else if flags.is_empty() {
                    return Err(error("`+` and `-` need at least one flag"));
                }
                for flag in flags.chars() {
                    let target = match flag {
                        'e' => &mut e,
                        'i' => &mut i,
                        'p' => &mut p,
                        _ => return Err(error("flags are `e`, `i` and `p`")),
                    };
                    if op == '-' {
                        *target &= !set;
                    } else {
                        *target |= set;
                    }
                }
            }
        }
        if e != 0 && e != (p | i) {
            return Err(error(
                "the effective set must be empty or cover every permitted and inheritable capability",
            ));
        }
        if p | i == 0 {
            return Err(error("no capability is permitted or inheritable"));
        }
        Ok(Self {
            permitted: p,
            inheritable: i,
            effective: e != 0,
        })
    }
}

#[cfg(test)]
#[path = "caps_tests.rs"]
mod tests;
