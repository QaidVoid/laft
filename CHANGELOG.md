
## [0.1.0] - 2026-10-29

### ⛰️  Features

- *(core)* List the files each trigger generates - ([ddcccff](https://github.com/QaidVoid/laft/commit/ddcccffbb8395b65c5fdea4580e5c134595a3715))
- *(core)* Add the trigger table and file capability parser - ([943f2a4](https://github.com/QaidVoid/laft/commit/943f2a473126f8de9f8993f172b9bac6b3a7c9c9))
- *(core)* Export the serde text form macro - ([3f8d1b9](https://github.com/QaidVoid/laft/commit/3f8d1b9f1fb6232117977be48345c03794cf34b0))
- *(core)* Match versions against dependency constraints - ([d7736d8](https://github.com/QaidVoid/laft/commit/d7736d8d1e106140429eb63356f78c634d66869e))
- *(core)* Add dependency grammar, system accounts and serde - ([54c4111](https://github.com/QaidVoid/laft/commit/54c4111c9722d42b45eb2432c398047be54246c1))
- *(core)* Add package identifiers, hashes and canonical JSON - ([f830bb4](https://github.com/QaidVoid/laft/commit/f830bb4b63675bb4a5595fb6dbdd3d3735e429d0))
- *(pkg)* Parse architectures from their text form - ([0528ac1](https://github.com/QaidVoid/laft/commit/0528ac155d2117917624ab42ef97ee80a849ca00))

### 🐛 Bug Fixes

- *(activate)* Reject unknown LUKS options when reading the config - ([7a9ea28](https://github.com/QaidVoid/laft/commit/7a9ea28eb539b1430083201a9b153dc097828988))
- *(boot)* Read boot.json through one shared lenient definition - ([27a30a4](https://github.com/QaidVoid/laft/commit/27a30a46a5baffb6d6aeeb2a2ab0db89c68026a6))
- *(core)* Require the boot state version and keep error causes - ([09b86c9](https://github.com/QaidVoid/laft/commit/09b86c92878bda9ec96b6671663a57682b7871ae))
- *(core)* Bound timestamps to the year 9999 - ([f66ad81](https://github.com/QaidVoid/laft/commit/f66ad811e8d1ead4688322374e8cde9c9681416a))
- *(core)* Keep version constraint lists non-empty - ([d833003](https://github.com/QaidVoid/laft/commit/d83300347dfde966898689a16aa6e175793950d7))
- *(core)* Name the canonical integer rule and tokenize safely - ([9a7db53](https://github.com/QaidVoid/laft/commit/9a7db53245347e6d1f1de3f66f7df1e2b7e6644a))
- *(core)* Keep canonical JSON a fixed point - ([ac06ee0](https://github.com/QaidVoid/laft/commit/ac06ee0e7c6c39225a2043255ad529a491c5daee))

### 🚜 Refactor

- *(core)* Share RFC 3339 timestamps - ([dee21fd](https://github.com/QaidVoid/laft/commit/dee21fd101b8afa20fe9ba49765365d8426dc6e3))

### 📚 Documentation

- *(core)* Say which trigger changes bump the table version - ([07e75f7](https://github.com/QaidVoid/laft/commit/07e75f7dc3c761b593aee8532dfa8c5934e5ac3b))

### 🧪 Testing

- *(core)* Cover <= and > constraints with revisions - ([08c4cf1](https://github.com/QaidVoid/laft/commit/08c4cf1c2e2081ac59f09f1fcf1ca9ebe2804b25))

### ⚙️ Miscellaneous Tasks

- Bootstrap cargo workspace - ([f4423d7](https://github.com/QaidVoid/laft/commit/f4423d7c63b7878b98e91c4d51617825a9934f68))

### Build

- Declare dependencies once in the workspace - ([c57dc30](https://github.com/QaidVoid/laft/commit/c57dc30940b71ce4f9b6baaecd4eb24a1ae2a030))
