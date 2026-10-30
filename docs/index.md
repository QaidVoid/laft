---
layout: page
title: Laft
titleTemplate: A Linux distribution you can roll back
sidebar: false
---

<Home>
<template #config>

```stav
system {
  include "./hardware.stav"

  hostname = "laft-vm"

  packages = [busybox:static, runit:static, limine, laft]

  repo core {
    url = "file:///repo"
    key = "RWTwpAdtxK4dSghH+5P46WIatF4FFuBMOlHyTe1lpy2BU1mZmhNHjZDf"
  }

  boot {
    cmdline   = ["console=ttyS0", "quiet", "laft.debug"]
    timeout   = 1
    microcode = force(off)
  }

  service vmtest-marker {
    description = "Print a marker line on the console"
    exec        = [
      "/usr/bin/sh",
      "-c",
      "echo 'laft-vmtest: marker service is running'; exec sleep 2147483647",
    ]
    log         = console
  }

  getty { ttys = ["ttyS0"] }
}
```

</template>
<template #plan>

```text
evaluating  /mnt/etc/laft/system.stav  2 files  12 ms
resolving   system
generation 1
  system
    + busybox:static  1.37.0-r1
    + laft            0.1.0-r1
    + limine          11.2.1-r1
    + linux           7.2.13-r1
    + runit:static    2.3.1-r1
  services
    + boot-complete
    + getty-ttys0
    + vmtest-marker
    ...
storing     5 packages
assembling  generation 1  135 ms
boot        entry 1 installed
activating  ok etc ok accounts
done        generation 1 is the default for the next boot
```

</template>
</Home>
