# ELF fixtures

Tiny ELF files built from the C sources here, without libc, and the expected ABI digest of `libversioned.so` computed with readelf, awk, sort and b3sum. Regenerate everything with `./build.sh`, which needs `cc` (GCC) with `-m32` support, binutils and `b3sum`, then commit the sources and the outputs together.
