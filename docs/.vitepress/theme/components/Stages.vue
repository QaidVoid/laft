<script setup lang="ts">
const stages = [
  {
    n: 0,
    name: 'A cross toolchain',
    where: 'on the host, then in a chroot',
    install: 'direct',
    body: 'A cross binutils, GCC and glibc, then temporary tools, built from the host without root into a fresh root with Laft’s merged /usr. Its recipes are a repository of their own and never become Laft packages.',
    check: 'A program cross compiled against the new glibc runs with the new loader.',
  },
  {
    n: 1,
    name: 'The base system as packages',
    where: 'inside that root',
    install: 'packages',
    body: 'Every recipe of the base system is built with the temporary toolchain into real Laft packages, which are unpacked into the root so that each later build uses them.',
    check: 'The kernel, Limine, runit, udev and dash as /bin/sh are all in place.',
  },
  {
    n: 2,
    name: 'A hermetic rebuild',
    where: 'in sandboxes, from the store',
    install: 'store',
    body: 'The stage 1 packages seed a new build-side store. Each build runs in a sandbox whose /usr holds only the build base and the declared dependencies, with no network. A seed stands in for its package until stage 2 has built it.',
    check: 'The stage 2 toolchain compiles and runs C and C++ in a view of the store.',
  },
  {
    n: 3,
    name: 'A second hermetic rebuild, with tests',
    where: 'seeded by stage 2 alone',
    install: 'store',
    body: 'Nothing from stage 1 reaches any view. Every package is rebuilt once more with its test suite, then rebuilt elsewhere and compared byte for byte.',
    check: 'laft-build check-repro',
  },
]
</script>

<template>
  <ol class="lf-stages" aria-label="The four stages of the bootstrap">
    <li v-for="s in stages" :key="s.n" class="stage" :class="`s${s.n}`">
      <div class="head">
        <span class="n" aria-hidden="true">{{ s.n }}</span>
        <div>
          <h4>Stage {{ s.n }}: {{ s.name }}</h4>
          <p class="where">{{ s.where }} <span class="mode">install = {{ s.install }}</span></p>
        </div>
      </div>
      <p class="body">{{ s.body }}</p>
      <p class="check"><span class="tag">checkpoint</span> <code v-if="s.n === 3">{{ s.check }}</code><template v-else>{{ s.check }}</template></p>
    </li>
  </ol>
</template>

<style scoped>
.lf-stages {
  list-style: none;
  margin: 28px 0 !important;
  padding: 0 !important;
  display: grid;
  gap: 10px;
  counter-reset: none;
}

.stage {
  position: relative;
  margin: 0 !important;
  padding: 16px 18px 14px 18px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 10px;
  background: var(--vp-c-bg-elv);
}

.stage + .stage::before {
  content: '';
  position: absolute;
  top: -11px;
  left: 34px;
  width: 2px;
  height: 10px;
  background: var(--vp-c-border);
}

.head {
  display: flex;
  gap: 14px;
  align-items: center;
}

.n {
  flex: none;
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 999px;
  font-family: var(--vp-font-family-mono);
  font-weight: 700;
  font-size: 14px;
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg-alt);
  border: 1px solid var(--vp-c-border);
}

.s2 .n,
.s3 .n {
  background: var(--vp-c-brand-soft);
  border-color: var(--vp-c-brand-3);
  color: var(--vp-c-brand-1);
}

.s3 .n {
  background: color-mix(in srgb, var(--lf-lichen-bright) 22%, transparent);
  border-color: var(--lf-lichen-bright);
  color: var(--lf-lichen);
}

h4 {
  margin: 0 !important;
  font-family: var(--lf-font-display);
  font-size: 17px;
  font-weight: 700;
  line-height: 1.3;
}

.where {
  margin: 2px 0 0 !important;
  font-size: 13px !important;
  line-height: 1.5 !important;
  color: var(--vp-c-text-3);
}

.mode {
  margin-left: 6px;
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 11px;
  color: var(--vp-c-text-2);
}

.body {
  margin: 10px 0 0 !important;
  font-size: 15px;
  line-height: 1.65 !important;
}

.check {
  margin: 8px 0 0 !important;
  font-size: 13.5px;
  line-height: 1.6 !important;
  color: var(--vp-c-text-2);
}

.tag {
  margin-right: 4px;
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--vp-c-text-3);
}
</style>
