
## [0.1.0] - 2026-10-29

### ⛰️  Features

- *(activate)* Start boot trials and mark generations good - ([229f6a6](https://github.com/QaidVoid/laft/commit/229f6a63761dc1f86666c3e79b103dccdec6f509))
- *(activate)* Install boot files and render the boot menu - ([cfe6c76](https://github.com/QaidVoid/laft/commit/cfe6c764a439d67396a81b12129c4ad953c133bd))
- *(activate)* Assemble generations with triggers and initramfs - ([2d4cbea](https://github.com/QaidVoid/laft/commit/2d4cbea97ef90d58f98b69cee3b8f642c705f760))
- *(activate)* Reconcile the live /etc and activate at boot - ([8e884a0](https://github.com/QaidVoid/laft/commit/8e884a0143efa8977d7dee7f7bcbeb17d73ad113))
- *(activate)* Render the static /etc tree of a generation - ([cc1e1d0](https://github.com/QaidVoid/laft/commit/cc1e1d0ecb67cf1b51c39097c7f43ed4a9cc3c50))
- *(activate)* Declare accounts and write the account databases - ([84d299c](https://github.com/QaidVoid/laft/commit/84d299c93ca2ed9e8136f8072dfb84199745d99e))

### 🐛 Bug Fixes

- *(activate)* Warn on every kept copy and refuse dotted /etc paths - ([ccc1e7a](https://github.com/QaidVoid/laft/commit/ccc1e7a701e691d0eeb7415efd29c0cc96047124))
- *(activate)* Tighten recorded ids, short shadow lines and admins - ([954f2d8](https://github.com/QaidVoid/laft/commit/954f2d8cb5bd09f908e9bfb30ff6674f81b0b845))
- *(activate)* Read the boot menu settings of older generations - ([7232eb2](https://github.com/QaidVoid/laft/commit/7232eb23d636cff4c1ac4c61b657ad80ff41a3d4))
- *(activate)* Read short account database lines with a warning - ([9d976c0](https://github.com/QaidVoid/laft/commit/9d976c0ab0b1d5e4e511887a5dc7d01d3299ae0e))
- *(activate)* Replace emptied directories Laft created in /etc - ([7dbd301](https://github.com/QaidVoid/laft/commit/7dbd3015b9a418e36487f501089bcaa0524aa683))
- *(activate)* Fall back to generic drivers without host modules - ([1754318](https://github.com/QaidVoid/laft/commit/1754318899e66c9c56277b3e2da0eca2daeec80e))
- *(activate)* Refuse explicit ids that a kept account holds - ([c8caf79](https://github.com/QaidVoid/laft/commit/c8caf79c642c2c3b4bd345fbedc968cb7ebf88ff))
- *(activate)* Never make unowned privileged copies setuid - ([9248d0e](https://github.com/QaidVoid/laft/commit/9248d0e33e00fcec141d4bedd065b178232f2b2c))
- *(activate)* Validate identity values, loader and TOML integers - ([02a0cfe](https://github.com/QaidVoid/laft/commit/02a0cfe28eef176396629eabd3ebb1a4313a5ac5))
- *(activate)* Write entries.json before limine.conf and sync both - ([48345f7](https://github.com/QaidVoid/laft/commit/48345f75895a0045150fbf0c2e88686adb2f63b3))
- *(activate)* Fail initramfs builds that lack required modules - ([3e16811](https://github.com/QaidVoid/laft/commit/3e16811a5c9b18a732010a17d1dac5613bb49013))
- *(activate)* Fingerprint /etc, detect rewrites and version triggers - ([78af7de](https://github.com/QaidVoid/laft/commit/78af7de22c79d87066be97cf71fd0fec74f6d8be))
- *(activate)* Make /etc resets keep backups and the live file - ([4772ed7](https://github.com/QaidVoid/laft/commit/4772ed712551c53584f4e5874fceba42f7a132d5))
- *(activate)* Harden account ids, paths, records and write order - ([fb42f57](https://github.com/QaidVoid/laft/commit/fb42f57394ede45e804d8bfde148207d69e940cb))
- *(activate)* Reject unknown LUKS options when reading the config - ([7a9ea28](https://github.com/QaidVoid/laft/commit/7a9ea28eb539b1430083201a9b153dc097828988))
- *(boot)* Read boot.json through one shared lenient definition - ([27a30a4](https://github.com/QaidVoid/laft/commit/27a30a46a5baffb6d6aeeb2a2ab0db89c68026a6))
- *(svc)* Write rendered entries through one shared writer - ([66c917b](https://github.com/QaidVoid/laft/commit/66c917b330184a004c3a2cb071540a3efe69f1c7))

### 🚜 Refactor

- *(activate)* Reuse write_json and document internal panics - ([6d228ab](https://github.com/QaidVoid/laft/commit/6d228abcf87114d236ed94af8dc421446a6a9db1))
- *(activate)* State index invariants and list the store once - ([d240813](https://github.com/QaidVoid/laft/commit/d2408137fde32c45abeb5f08f6dc4f91a8c49203))

### 🧪 Testing

- *(activate)* Check boot trials with the laft-boot decision - ([5118963](https://github.com/QaidVoid/laft/commit/5118963988e383318d9f5e8b8d173e86ffd4a70c))
- *(activate)* Cover trigger failures, state records and cycles - ([d33c819](https://github.com/QaidVoid/laft/commit/d33c819f6b868e61e12b65d4cf9ba5904b40a55d))
- Remove read-only test trees through a shared helper - ([747454a](https://github.com/QaidVoid/laft/commit/747454a427b3207581b79e93b53904710ab58253))
