
## [0.1.0] - 2026-10-29

### ⛰️  Features

- *(pkg)* Parse architectures from their text form - ([0528ac1](https://github.com/QaidVoid/laft/commit/0528ac155d2117917624ab42ef97ee80a849ca00))
- *(pkg)* Write and read package files - ([bdfe59a](https://github.com/QaidVoid/laft/commit/bdfe59a599d7d1902ce340f72e35bd249899ce63))
- *(pkg)* Add payload manifests, metadata and store ids - ([799164b](https://github.com/QaidVoid/laft/commit/799164bab8bb14e3d027292744afbb6d4bc613bc))

### 🐛 Bug Fixes

- *(pkg)* Let write_package borrow its writer - ([33fa3cd](https://github.com/QaidVoid/laft/commit/33fa3cd1be6932ef93f8541239dd78d06c02b98e))
- *(pkg)* Say what license validation checks - ([13ae94a](https://github.com/QaidVoid/laft/commit/13ae94aa0c10e7ff183f9fb7572c64a91e32ba93))
- *(pkg)* Keep the cause of reader I/O errors - ([4555b9e](https://github.com/QaidVoid/laft/commit/4555b9ed52d46ce8c303dd22cd17bafd85685d46))

### 📚 Documentation

- *(pkg)* Document the panics of the JSON writers - ([009eb46](https://github.com/QaidVoid/laft/commit/009eb460278a7d5855128e1a209499dd3b50f006))
- *(pkg)* Correct the manifest owner and metadata validation docs - ([ebfb803](https://github.com/QaidVoid/laft/commit/ebfb8037777bac4d65cd37b7c69df1d61340f050))

### 🧪 Testing

- *(pkg)* Check that path rules accept and reject paths - ([7cfe161](https://github.com/QaidVoid/laft/commit/7cfe1613751e994fb438681bef79cd9b60e014ec))
- Remove read-only test trees through a shared helper - ([747454a](https://github.com/QaidVoid/laft/commit/747454a427b3207581b79e93b53904710ab58253))

### Build

- Declare dependencies once in the workspace - ([c57dc30](https://github.com/QaidVoid/laft/commit/c57dc30940b71ce4f9b6baaecd4eb24a1ae2a030))
