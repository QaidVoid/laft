
## [0.1.0] - 2026-10-29

### ⛰️  Features

- *(boot)* Add the laft-boot initramfs init - ([db94312](https://github.com/QaidVoid/laft/commit/db94312cc7675417352747e43879f904bcff7679))

### 🐛 Bug Fixes

- *(activate)* Reject unknown LUKS options when reading the config - ([7a9ea28](https://github.com/QaidVoid/laft/commit/7a9ea28eb539b1430083201a9b153dc097828988))
- *(boot)* Keep the trial when the menu lacks the fallback entry - ([9eeab4a](https://github.com/QaidVoid/laft/commit/9eeab4aee2db718d0b97e5ffa442e1917348bfda))
- *(boot)* Retry the switch root steps before the initramfs empties - ([7c62b2a](https://github.com/QaidVoid/laft/commit/7c62b2ad36d7558564a25b15d850556270469068))
- *(boot)* End the plan when the default link cannot be set - ([92c57a9](https://github.com/QaidVoid/laft/commit/92c57a93a2b6da154d42c0e0374089c209adda3a))
- *(boot)* Verify the menu entry before setting the default - ([83cee41](https://github.com/QaidVoid/laft/commit/83cee41cc0592e050d71b0aa875039f589e63572))
- *(boot)* Split parameters like the kernel and refuse . as a device - ([c08b299](https://github.com/QaidVoid/laft/commit/c08b2993dd897b3872948d4cdfe0a64625fe8813))
- *(boot)* Retry only steps that can run again - ([169c4c4](https://github.com/QaidVoid/laft/commit/169c4c4662d76c103e2672ace9e1f417693a1ea9))
- *(boot)* Find each LUKS device through its own spec - ([827155f](https://github.com/QaidVoid/laft/commit/827155f838bb96197d8ca9ba01c2b1a27b8525db))
- *(boot)* Keep boot history when generations cannot be listed - ([b57ea48](https://github.com/QaidVoid/laft/commit/b57ea48db8b503489e8b8e39b01cd2c842df8479))
- *(boot)* Keep a fallback in effect on every later boot - ([899147e](https://github.com/QaidVoid/laft/commit/899147eed9381710481052b9f769362542df1c15))
- *(boot)* Read boot.json through one shared lenient definition - ([27a30a4](https://github.com/QaidVoid/laft/commit/27a30a46a5baffb6d6aeeb2a2ab0db89c68026a6))

### 🚜 Refactor

- *(boot)* Move the boot into a library behind a thin binary - ([0a05acf](https://github.com/QaidVoid/laft/commit/0a05acfe5ac36be5913d1d5d6e746967e2d9736a))
- *(boot)* Pair LUKS devices with nodes in the config order - ([10a4e1a](https://github.com/QaidVoid/laft/commit/10a4e1a40e370fc58e5ab1aa1e2122add803a852))

### 🧪 Testing

- Remove read-only test trees through a shared helper - ([747454a](https://github.com/QaidVoid/laft/commit/747454a427b3207581b79e93b53904710ab58253))
