# Laft

Laft is a Linux distribution built from its own recipes. Packages live in a content addressed store, every `/usr` is a view assembled from that store, and the whole system is described by one declarative configuration and switches between immutable generations atomically. It uses glibc and runit, and all of its tooling is written in Rust. It is pre-alpha: nothing here is usable yet.
