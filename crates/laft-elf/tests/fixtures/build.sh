#!/bin/sh
# Rebuilds the ELF fixtures and the expected ABI digest of libversioned.so.
#
# The expected digest is computed without laft-elf: readelf lists the dynamic
# symbols, awk turns the exported ones into ABI lines, sort orders and
# deduplicates them bytewise, and b3sum hashes the result. It does not escape
# names, so it only holds for names without a backslash, space or `@`.
set -eu
cd "$(dirname "$0")"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fixtures=$(pwd)

flags="-O2 -nostdlib -fno-asynchronous-unwind-tables -s -Wl,--build-id=sha1 -Wl,-z,noseparate-code"

# shellcheck disable=SC2086
cc $flags -shared -fPIC -o libversioned.so libversioned.c \
  -Wl,-soname,libversioned.so.1 -Wl,--version-script=libversioned.map \
  -Wl,--disable-new-dtags,-rpath,/opt/legacy/lib
# The runpath must reach the linker with a literal $ORIGIN.
# shellcheck disable=SC2016,SC2086
cc $flags -fPIE -pie -o program program.c -L. -l:libversioned.so \
  -Wl,--enable-new-dtags,-rpath,'$ORIGIN/../lib:/usr/lib'
# shellcheck disable=SC2086
cc $flags -static -o static static.c
# shellcheck disable=SC2086
cc $flags -fPIE -static-pie -o static-pie static.c

# A library without a soname, linked by a relative path, is recorded in
# DT_NEEDED and in the version requirements as that path.
# shellcheck disable=SC2086
cc $flags -shared -fPIC -o "$tmp/libversioned.so" libversioned.c \
  -Wl,--version-script=libversioned.map
# shellcheck disable=SC2086
(cd "$tmp" && cc $flags -fPIE -pie -o "$fixtures/needs-path" \
  "$fixtures/program.c" ./libversioned.so)

# A 32-bit program needs a 32-bit library to link against.
# shellcheck disable=SC2086
cc -m32 $flags -shared -fPIC -o "$tmp/libversioned32.so" libversioned.c \
  -Wl,-soname,libversioned.so.1 -Wl,--version-script=libversioned.map
# shellcheck disable=SC2086
cc -m32 $flags -fPIE -pie -o program32 program32.c "$tmp/libversioned32.so"

readelf --version-info --wide libversioned.so > "$tmp/versions"
# readelf names GNU_IFUNC only for objects marked with the GNU OS ABI.
readelf --dyn-syms --wide libversioned.so \
  | sed 's/<OS specific>: 10/IFUNC/' > "$tmp/symbols"

# The version suffix comes from the raw .gnu.version table, because readelf
# leaves it off some symbols when it prints them. A table row holds four
# hexadecimal version indexes, each followed by h when hidden and by the
# version name in parentheses, for example
# "  004:   1 (*global*)      2h(VERS_1)      c (VERS_2)      1h".
# readelf prints large sizes in hexadecimal, so they are converted.
awk '
  function decimal(size,  digits, value, i) {
    if (size !~ /^0x/) return size
    digits = substr(size, 3)
    value = 0
    for (i = 1; i <= length(digits); i++)
      value = value * 16 + index("0123456789abcdef", substr(digits, i, 1)) - 1
    return value
  }
  FNR == NR {
    if ($0 ~ /^Version symbols section/) { table = 1; next }
    if ($0 ~ /^Version (definition|needs) section/) { table = 0; next }
    if (!table || $1 !~ /^[0-9a-f]+:$/) next
    row = $0
    sub(/^ *[0-9a-f]+:/, "", row)
    gsub(/\(/, " (", row)
    fields = split(row, token, " ")
    for (i = 1; i <= fields; i++) {
      if (token[i] ~ /^\(/) {
        version = substr(token[i], 2, length(token[i]) - 2)
        if (number != "0" && number != "1")
          suffix[count - 1] = (hidden ? "@" : "@@") version
        continue
      }
      number = token[i]
      hidden = sub(/h$/, "", number)
      suffix[count++] = ""
    }
    next
  }
  $1 ~ /^[0-9]+:$/ && $7 != "UND" \
    && ($5 == "GLOBAL" || $5 == "WEAK" || $5 == "UNIQUE") \
    && ($6 == "DEFAULT" || $6 == "PROTECTED") {
    index_ = $1
    sub(/:$/, "", index_)
    name = $8
    sub(/@.*$/, "", name)
    name = name suffix[index_ + 0]
    if ($4 == "FUNC" || $4 == "IFUNC") print "F " name
    else if ($4 == "OBJECT") print "O " name " " decimal($3)
    else if ($4 == "TLS") print "T " name " " decimal($3)
    else if ($4 == "NOTYPE") print "N " name
  }' "$tmp/versions" "$tmp/symbols" | LC_ALL=C sort -u > libversioned.abi
b3sum --no-names libversioned.abi > libversioned.abi.b3
