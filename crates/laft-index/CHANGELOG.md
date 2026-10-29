
## [0.1.0] - 2026-10-29

### ⛰️  Features

- *(index)* Add signed indexes and the client trust model - ([da8e7a7](https://github.com/QaidVoid/laft/commit/da8e7a7de480f5aa5a8853d9de1f87ea5e7c4986))

### 🐛 Bug Fixes

- *(fetch)* Report a cached index revocation even if removal fails - ([8a42349](https://github.com/QaidVoid/laft/commit/8a4234950407c2d0141704161a703e0d0b6ffb3e))
- *(fetch)* Harden resumes, keys.json lookup and the index cache - ([a8b61a6](https://github.com/QaidVoid/laft/commit/a8b61a6a565c6556620fe8b3e08a41ae5add6994))
- *(index)* Reject versioned pkg-config provider defaults - ([8b37a2c](https://github.com/QaidVoid/laft/commit/8b37a2cea9d32927f0841c63913db32557ec5a41))
- *(index)* Report unwritable state records instead of panicking - ([79da6be](https://github.com/QaidVoid/laft/commit/79da6be5324e79434274f5637bd98a0b7ba942fc))
- *(index)* Validate yanks like packages - ([05e7c77](https://github.com/QaidVoid/laft/commit/05e7c779ffef3c23b813253f58c320749a34320a))
- *(index)* Let a generation floor only lower accepted generations - ([9ee0580](https://github.com/QaidVoid/laft/commit/9ee058098479f04f3944ef6c0ce84b3497f40c3a))
- *(index)* Record the root key that verified keys.json - ([283b37c](https://github.com/QaidVoid/laft/commit/283b37cacf45e1fc5d42b7132674a2dd7d7434b4))

### 🚜 Refactor

- *(index)* Drop the unread signing time from index records - ([92b5930](https://github.com/QaidVoid/laft/commit/92b59303e5b622689139623a92ab412e3615c6ba))
- *(index)* Share the serde text form macro from laft-core - ([651b4b1](https://github.com/QaidVoid/laft/commit/651b4b195e65e94e94031c7b39f7a3120704afce))
- *(index)* Parse the comment architecture with Arch::from_str - ([70b4537](https://github.com/QaidVoid/laft/commit/70b453780cbc903f3917ca821b4eea429c252718))

### 🧪 Testing

- *(index)* Round-trip generated indexes and keys.json files - ([148d547](https://github.com/QaidVoid/laft/commit/148d54715a3c2a24bb1c34b873e2842e32a3fde3))
- Remove read-only test trees through a shared helper - ([747454a](https://github.com/QaidVoid/laft/commit/747454a427b3207581b79e93b53904710ab58253))
