
## [0.1.0] - 2026-10-29

### ⛰️  Features

- *(store)* Stage generations, carry kernel modules and record boot - ([a79e7fa](https://github.com/QaidVoid/laft/commit/a79e7fa044a83b9702a45746e0dc64c1256f349f))
- *(store)* Collect unreferenced generations, views and entries - ([f6e1d4d](https://github.com/QaidVoid/laft/commit/f6e1d4dec51793420b8d463f94cc5cd5a6eb8999))
- *(store)* Add the store, views, generations and transactions - ([2bed64b](https://github.com/QaidVoid/laft/commit/2bed64ba853f765880877833323b35bc7fa21ca2))

### 🐛 Bug Fixes

- *(core)* Bound timestamps to the year 9999 - ([f66ad81](https://github.com/QaidVoid/laft/commit/f66ad811e8d1ead4688322374e8cde9c9681416a))
- *(pkg)* Let write_package borrow its writer - ([33fa3cd](https://github.com/QaidVoid/laft/commit/33fa3cd1be6932ef93f8541239dd78d06c02b98e))
- *(store)* Report the right file and avoid panics in records - ([274f6e8](https://github.com/QaidVoid/laft/commit/274f6e8e5de283acbcea9052c16ebaf8ba873bf8))
- *(store)* Treat an empty boot id as unknown during recovery - ([2cd9fe1](https://github.com/QaidVoid/laft/commit/2cd9fe1e46ba37668c62e775c909b6d69de0025e))
- *(store)* Fail garbage collection on unreadable records - ([8c6047f](https://github.com/QaidVoid/laft/commit/8c6047f8d624b40d241b801e1b1831e53a012791))
- *(store)* Reject journals that name paths outside the store - ([dbeb57c](https://github.com/QaidVoid/laft/commit/dbeb57c113bef2941b2501aa1563266d82f208f9))

### 🚜 Refactor

- *(core)* Share RFC 3339 timestamps - ([dee21fd](https://github.com/QaidVoid/laft/commit/dee21fd101b8afa20fe9ba49765365d8426dc6e3))
- *(store)* Apply the alternatives the resolver chose - ([7905a55](https://github.com/QaidVoid/laft/commit/7905a5553c03997e8b6489a5756745ea6822167c))
- *(store)* Type record times and drop unread journal fields - ([4d57801](https://github.com/QaidVoid/laft/commit/4d57801ae2f2f6b000a95216c3852231e02d26fe))

### 🧪 Testing

- Remove read-only test trees through a shared helper - ([747454a](https://github.com/QaidVoid/laft/commit/747454a427b3207581b79e93b53904710ab58253))
