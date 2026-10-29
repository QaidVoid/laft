
## [0.1.0] - 2026-10-29

### ⛰️  Features

- *(svc)* Add the service launcher - ([8521606](https://github.com/QaidVoid/laft/commit/85216068c9086cfdf5b4d611ab1510b1df8ea95a))
- *(svc)* Plan live reconciliation between generations - ([07152d1](https://github.com/QaidVoid/laft/commit/07152d189e4754071147b9c316f42e00c5f4c44f))
- *(svc)* Render services, stages and entry links for runit - ([4a2e547](https://github.com/QaidVoid/laft/commit/4a2e547121ae9d4f9ace6ed9ec7e836afdfa2642))
- *(svc)* Add the service model, validation and spec files - ([b55a552](https://github.com/QaidVoid/laft/commit/b55a552fd00c92b2851e0cc7ef8220219f800ac2))

### 🐛 Bug Fixes

- *(svc)* Reject a shared directory declared with another mode - ([4167040](https://github.com/QaidVoid/laft/commit/416704034c82fd1980b61e7c99e69e393a48c8f3))
- *(svc)* Hash specs the same way when rendering and loading - ([a56c22a](https://github.com/QaidVoid/laft/commit/a56c22a052a1e774fd79050f1205d6b0c7575ba7))
- *(svc)* Name gettys of serial terminals in lowercase - ([45bbf15](https://github.com/QaidVoid/laft/commit/45bbf157d7520f9074975769b7f1f3a19318a3d2))
- *(svc)* Never wait less than the default at shutdown - ([e1bf27e](https://github.com/QaidVoid/laft/commit/e1bf27ede4287529d03121895196d3cc7b69bd13))
- *(svc)* Write rendered entries through one shared writer - ([66c917b](https://github.com/QaidVoid/laft/commit/66c917b330184a004c3a2cb071540a3efe69f1c7))
- *(svc)* Refuse dot components in launcher directory paths - ([2dd5bc3](https://github.com/QaidVoid/laft/commit/2dd5bc356866e58c748b8e4e6720da8c6534352a))
- *(svc)* Reject environment entries a program cannot receive - ([d161076](https://github.com/QaidVoid/laft/commit/d1610762e7c3ae7d134325eb742f56a7f5b2bbad))
- *(svc)* Restore the launcher's limits after starting a command - ([369cede](https://github.com/QaidVoid/laft/commit/369cede5d3d7adc9c28efefda636a4988f89eafc))
- *(svc)* Apply declared groups to services that run as root - ([a743661](https://github.com/QaidVoid/laft/commit/a7436615ea490b019dc1b844da057c991038361c))
- *(svc)* Honor restart never over the reserved exit statuses - ([16a7ee4](https://github.com/QaidVoid/laft/commit/16a7ee482ebc40c6e23c8e2123669b17fe93c576))
- *(svc)* Keep early services out of live stops and starts - ([7eab541](https://github.com/QaidVoid/laft/commit/7eab54175c5493dd352bbf04d6c99e19aa24b721))
- *(svc)* Kill a readiness check that outlives the timeout - ([cfa0b39](https://github.com/QaidVoid/laft/commit/cfa0b3929c385a9c41befb1f9614a8a4999b3662))
- *(svc)* Mark an early immediate service ready only after exec - ([a6baca6](https://github.com/QaidVoid/laft/commit/a6baca6f8b1e3db8571dc3f01e6ba24c6929486f))
- *(svc)* Never signal a process through a stale early pid file - ([20e7047](https://github.com/QaidVoid/laft/commit/20e70474d7814cc80af2893ed15e72e076fcab82))
- *(svc)* Always mark early ready after the early oneshots - ([8fd4fea](https://github.com/QaidVoid/laft/commit/8fd4fea35a43aa1c4bfe0985f4e0d3ee5d6eb4ab))
- *(svc)* Stop supervised services with their stop signal - ([959916d](https://github.com/QaidVoid/laft/commit/959916d76eb2d821580a18127e23175a53fb8746))
- *(svc)* Take runit's finish arguments and derive the service dir - ([0f717eb](https://github.com/QaidVoid/laft/commit/0f717eb269621e4345c83a93e835cdd2c49a27a5))

### 📚 Documentation

- *(svc)* Document notify readiness and account parsing exactly - ([901887d](https://github.com/QaidVoid/laft/commit/901887daa4cdce2885554eee483349e68f77b1cf))
- *(svc)* Explain why per-thread privilege drops are sound - ([1bc0e57](https://github.com/QaidVoid/laft/commit/1bc0e578d820393364ec535bfcb07b3ea1fc2cb0))

### 🧪 Testing

- *(svc)* Drive Launcher::exec for a longrun in a child process - ([ac201f3](https://github.com/QaidVoid/laft/commit/ac201f382ee42b845d70541e600e7e40c500781e))
- *(svc)* Assert the outcome of finishing a failed oneshot - ([ea6d044](https://github.com/QaidVoid/laft/commit/ea6d044503fb828f75cba7edce273bff2c0aa45e))
- *(svc)* Serialize launcher tests that share the umask - ([8ac9ef3](https://github.com/QaidVoid/laft/commit/8ac9ef34b64caf67b54a5fda7276be24c575c6a5))
- Remove read-only test trees through a shared helper - ([747454a](https://github.com/QaidVoid/laft/commit/747454a427b3207581b79e93b53904710ab58253))
